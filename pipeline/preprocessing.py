"""Photo -> rectified working-resolution map image.

Steps: raw (non-EXIF-rotated) load -> downscale -> paper-frame detection &
perspective rectification -> best-effort magnetic-north-line detection
(local scale anchor). Real photos, not scans: perspective, glare, shadows,
creases are expected, so every geometric step has a documented fallback
rather than raising on the happy-path assumption.
"""

from __future__ import annotations

from dataclasses import dataclass

import cv2
import numpy as np

from pipeline.config import WORKING_MAX_SIDE


@dataclass
class PreprocessResult:
    source_path: str
    image: np.ndarray                # rectified, working-resolution BGR image
    scale_to_original: float         # multiply working-res px by this to get original-photo px
    quad_found: bool                 # False => paper-frame detection fell back to full frame
    mn_line_xs: list[float]          # x positions (working-res px) of detected magnetic-north lines
    mn_line_spacing_px: float | None  # median spacing between them, None if <2 found


def load_image_exif_safe(path: str) -> np.ndarray:
    """Load a JPEG, return BGR uint8 ndarray.

    Deliberately does NOT apply the file's EXIF orientation tag. Checked
    directly against this project's test photos: the raw pixel data is
    already upright (title readable, map north-ish-up) while the EXIF
    orientation tags are inconsistent (6 on most files, 3 on one) and, when
    applied, rotate the *correctly-oriented* raw pixels into a wrong
    orientation -- i.e. the tags are stale/wrong for this dataset, not the
    pixels. Reading orientation doesn't matter to this pipeline anyway:
    find_paper_quad below establishes the working frame from the paper's own
    edges, not from metadata, so this is robust regardless of what a given
    photo's EXIF claims.

    Note: cv2.imread applies EXIF orientation automatically as of OpenCV 4.5+,
    which is exactly the behaviour we're opting out of here -- pass
    IMREAD_IGNORE_ORIENTATION explicitly to get genuinely raw pixels.
    """
    return cv2.imread(path, cv2.IMREAD_COLOR | cv2.IMREAD_IGNORE_ORIENTATION)


def white_balance(img: np.ndarray, percentile: float = 97.0) -> np.ndarray:
    """Normalize color cast using the photographed paper's own white margin
    as a reference (percentile white-patch balance): find the brightest
    pixels in the frame (almost certainly the white paper background, not
    map ink) and scale each channel so their average becomes neutral.

    Real photos of these maps carry a strong, inconsistent color cast from
    whatever indoor lighting they were shot under -- checked directly:
    map4.jpg's paper background averages BGR (185, 196, 217), ~30 points
    short of neutral on blue -- which pushes every warm map color (browns,
    tans) further toward "red" and collides with the course-overprint hue
    range in course_detection.py. A *fixed* gain correction wouldn't work
    here for the same reason fixed HSV thresholds don't (per-photo lighting
    varies too much) -- this instead measures and corrects each photo's own
    cast from its own paper.
    """
    gray = cv2.cvtColor(img, cv2.COLOR_BGR2GRAY)
    threshold = np.percentile(gray, percentile)
    white_mask = gray >= threshold
    if white_mask.sum() < 100:
        return img
    b, g, r = cv2.split(img.astype(np.float32))
    mb, mg, mr = b[white_mask].mean(), g[white_mask].mean(), r[white_mask].mean()
    target = (mb + mg + mr) / 3.0
    b *= target / max(mb, 1e-6)
    g *= target / max(mg, 1e-6)
    r *= target / max(mr, 1e-6)
    return np.clip(cv2.merge([b, g, r]), 0, 255).astype(np.uint8)


def downscale(img: np.ndarray, max_side: int = WORKING_MAX_SIDE) -> tuple[np.ndarray, float]:
    """Resize so the longer side is <= max_side. Returns (small_img, scale).

    scale = small_size / original_size (<= 1.0). To map a working-res
    coordinate back to the original photo: orig = working / scale.
    """
    h, w = img.shape[:2]
    longer = max(h, w)
    if longer <= max_side:
        return img, 1.0
    scale = max_side / longer
    small = cv2.resize(img, (int(round(w * scale)), int(round(h * scale))), interpolation=cv2.INTER_AREA)
    return small, scale


def _order_corners(pts: np.ndarray) -> np.ndarray:
    """Order 4 points as top-left, top-right, bottom-right, bottom-left."""
    pts = pts.reshape(4, 2).astype(np.float32)
    s = pts.sum(axis=1)
    diff = np.diff(pts, axis=1).reshape(-1)
    tl = pts[np.argmin(s)]
    br = pts[np.argmax(s)]
    tr = pts[np.argmin(diff)]
    bl = pts[np.argmax(diff)]
    return np.array([tl, tr, br, bl], dtype=np.float32)


def find_paper_quad(img: np.ndarray) -> np.ndarray | None:
    """Find the 4 corners of the photographed paper sheet.

    Largest-contour + polygon approximation against the table background.
    Returns ordered (tl, tr, br, bl) points in image pixel coords, or None if
    no confident quad was found (caller falls back to using the full frame).
    """
    gray = cv2.cvtColor(img, cv2.COLOR_BGR2GRAY)
    blurred = cv2.GaussianBlur(gray, (5, 5), 0)
    edges = cv2.Canny(blurred, 40, 120)
    edges = cv2.dilate(edges, np.ones((5, 5), np.uint8), iterations=2)

    contours, _ = cv2.findContours(edges, cv2.RETR_EXTERNAL, cv2.CHAIN_APPROX_SIMPLE)
    if not contours:
        return None
    largest = max(contours, key=cv2.contourArea)
    img_area = img.shape[0] * img.shape[1]
    if cv2.contourArea(largest) < 0.25 * img_area:
        # Sheet doesn't dominate the frame enough to trust this contour.
        return None

    # approxPolyDP on the *raw* contour, not its convex hull: verified
    # directly against map2.jpg/map4.jpg -- a shadow or low-contrast patch
    # near one corner puts a concave notch in the raw contour there, and
    # approxPolyDP happily returns a "clean" 4-gon that cuts straight across
    # the notch instead of reaching the true corner, silently discarding a
    # wedge of real map content (confirmed by cross-checking the found quad
    # against the actual photo). The hull bridges that kind of notch.
    hull = cv2.convexHull(largest)
    hull_area = cv2.contourArea(hull)
    peri = cv2.arcLength(hull, True)
    quad = None
    for eps_frac in (0.01, 0.02, 0.03, 0.05, 0.08):
        approx = cv2.approxPolyDP(hull, eps_frac * peri, True)
        if len(approx) == 4:
            quad = approx
            break

    # Even off the hull, approxPolyDP can still return a too-simple 4-gon
    # that undercuts the true boundary -- checked directly: this ratio was
    # ~0.69 on both map2.jpg and map4.jpg where a corner got cut off, vs.
    # >0.9 on files where the quad was fine. Below the threshold, prefer the
    # rotated bounding rectangle: it may include a sliver of table
    # background, but that's a far safer failure than silently cropping out
    # real map content.
    if quad is not None and cv2.contourArea(quad) >= 0.85 * hull_area:
        return _order_corners(quad)

    rect = cv2.minAreaRect(hull)
    box = cv2.boxPoints(rect)
    return _order_corners(box)


def rectify(img: np.ndarray, quad: np.ndarray) -> np.ndarray:
    """Perspective-warp `img` so `quad` maps onto an axis-aligned rectangle."""
    tl, tr, br, bl = quad
    width_top = np.linalg.norm(tr - tl)
    width_bottom = np.linalg.norm(br - bl)
    height_left = np.linalg.norm(bl - tl)
    height_right = np.linalg.norm(br - tr)
    out_w = int(round(max(width_top, width_bottom)))
    out_h = int(round(max(height_left, height_right)))
    out_w, out_h = max(out_w, 1), max(out_h, 1)

    dst = np.array([[0, 0], [out_w - 1, 0], [out_w - 1, out_h - 1], [0, out_h - 1]], dtype=np.float32)
    m = cv2.getPerspectiveTransform(quad, dst)
    return cv2.warpPerspective(img, m, (out_w, out_h))


def _all_segments(lines: np.ndarray | None, min_len: float) -> list[tuple[float, float, float, float]]:
    """Return (angle_deg_mod_180, x_mid, y_mid, length) for every long-enough
    HoughLinesP segment, with no angle restriction -- rectify() only aligns
    the *paper edges* to the frame, not the map's reading orientation, so the
    magnetic-north lines can come out horizontal just as easily as vertical
    depending on which corner the corner-ordering heuristic called "top-left".
    """
    out = []
    if lines is None:
        return out
    for seg in lines:
        # OpenCV has varied between returning (N, 1, 4) and (N, 4) across
        # versions/bindings for HoughLinesP; handle both.
        x1, y1, x2, y2 = np.asarray(seg).reshape(-1).astype(float)
        dx, dy = x2 - x1, y2 - y1
        length = float(np.hypot(dx, dy))
        if length < min_len:
            continue
        angle = float(np.degrees(np.arctan2(dy, dx)) % 180.0)  # 0=horizontal, 90=vertical
        out.append((angle, (x1 + x2) / 2.0, (y1 + y2) / 2.0, length))
    return out


def _drop_border_segments(
    segments: list[tuple[float, float, float, float]], img_shape: tuple[int, int], margin_frac: float = 0.03
) -> list[tuple[float, float, float, float]]:
    """Drop segments whose midpoint sits within `margin_frac` of any image
    edge. The rectified sheet's own outer border/frame otherwise dominates
    Hough results (it's the single longest, straightest edge in the photo)
    and would get mistaken for part of the magnetic-north-line family.
    """
    h, w = img_shape[:2]
    mx, my = w * margin_frac, h * margin_frac
    return [s for s in segments if mx <= s[1] <= w - mx and my <= s[2] <= h - my]


def _mask_edges(mask: np.ndarray) -> np.ndarray:
    """Thin a filled color mask down to boundary edges before Hough.

    Feeding a filled blob (e.g. a lake) straight into HoughLinesP makes every
    row of interior pixels look like a candidate line; running on the mask's
    boundary instead means a lake contributes its (curved, non-repeating)
    shoreline rather than a false family of parallel lines.
    """
    mask = cv2.morphologyEx(mask, cv2.MORPH_CLOSE, np.ones((7, 7), np.uint8))
    return cv2.Canny(mask, 50, 150)


def detect_magnetic_north_lines(img: np.ndarray, tol_deg: float = 10.0) -> tuple[list[float], float | None]:
    """Best-effort detection of the equally-spaced magnetic-north lines.

    Color-agnostic (grayscale Canny -> Hough), restricted to near-vertical
    segments: find_paper_quad + rectify already leave the sheet close to
    axis-aligned, so this is a safer assumption than searching for a dominant
    line family at *any* angle -- that was tried and, on real photos full of
    other straight linework (building edges, hatching, paths), it just as
    often locks onto a spurious diagonal family as the real one. Getting this
    wrong would rotate an already-correct image into a wrong one, which is a
    worse outcome than simply not finding the lines.

    This is explicitly a best-effort component (see plan): the real photos
    interrupt these thin printed lines constantly (contours, paths, course
    overprint, text crossing them), so returning ([], None) is common and
    does not block the rest of the pipeline -- spacing is only ever used as
    an optional local-scale hint.

    Returns (sorted x-positions px, median spacing px | None if <2 lines).
    """
    gray = cv2.cvtColor(img, cv2.COLOR_BGR2GRAY)
    edges = cv2.Canny(gray, 30, 90)
    lines = cv2.HoughLinesP(edges, 1, np.pi / 360, threshold=50,
                             minLineLength=img.shape[0] * 0.06, maxLineGap=90)
    segs = _drop_border_segments(_all_segments(lines, min_len=img.shape[0] * 0.06), img.shape)

    near_vertical = [s for s in segs if abs(((s[0] - 90.0 + 90.0) % 180.0) - 90.0) <= tol_deg]
    if len(near_vertical) < 3:
        return [], None

    near_vertical.sort(key=lambda s: s[1])
    clusters: list[list[tuple[float, float, float, float]]] = [[near_vertical[0]]]
    for s in near_vertical[1:]:
        if s[1] - clusters[-1][-1][1] <= img.shape[1] * 0.01:
            clusters[-1].append(s)
        else:
            clusters.append([s])

    # A real magnetic-north line gets chopped into many short Hough segments
    # by everything crossing it (contours, paths, course overprint, text),
    # but those fragments collectively span most of the map's height. A
    # stray feature that merely happens to have a near-vertical edge (a
    # building corner, a lake shore) contributes only one or two segments
    # over a small y-range -- require both a minimum member count and a
    # minimum vertical span to tell the two apart.
    img_h = img.shape[0]
    centers = sorted(
        float(np.mean([s[1] for s in cluster]))
        for cluster in clusters
        if len(cluster) >= 3 and (max(s[2] for s in cluster) - min(s[2] for s in cluster)) >= 0.25 * img_h
    )
    if len(centers) < 3:
        return centers, None
    spacings = np.diff(centers)
    median_spacing = float(np.median(spacings))
    keep = [centers[0]]
    for x, sp in zip(centers[1:], spacings):
        if abs(sp - median_spacing) / max(median_spacing, 1e-6) < 0.4:
            keep.append(x)
    spacing_out = float(np.median(np.diff(keep))) if len(keep) >= 2 else None
    return keep, spacing_out


def preprocess_image(path: str, max_side: int = WORKING_MAX_SIDE) -> PreprocessResult:
    full = load_image_exif_safe(path)
    small, scale = downscale(full, max_side)

    quad = find_paper_quad(small)
    if quad is not None:
        rectified = rectify(small, quad)
        quad_found = True
    else:
        rectified = small
        quad_found = False

    rectified = white_balance(rectified)
    mn_xs, mn_spacing = detect_magnetic_north_lines(rectified)

    return PreprocessResult(
        source_path=path,
        image=rectified,
        scale_to_original=scale,
        quad_found=quad_found,
        mn_line_xs=mn_xs,
        mn_line_spacing_px=mn_spacing,
    )
