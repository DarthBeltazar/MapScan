"""Rectified map image -> course overprint (controls, start, finish, legs).

The printed course color is NOT a fixed hue across maps (checked against all
9 test photos): red, dark maroon, and pink/magenta all show up depending on
the event. This module casts a broad warm-hue net for "course ink" and then
classifies connected components by *shape* (circle/triangle/thin-line), not
by exact color, which is what actually stays constant across maps -- see
plan's Context section for the full finding.
"""

from __future__ import annotations

import re
import shutil
from dataclasses import dataclass, field
from pathlib import Path

import cv2
import numpy as np
from shapely.geometry import LineString

from pipeline.config import (
    HOUGH_PARAM2,
    HOUGH_PARAM2_DEFAULT,
    RING_COVERAGE_MIN_FRACTION,
    START_TRIANGLE_AREA_RATIO_RANGE,
    START_TRIANGLE_MIN_EQUILATERAL_SCORE,
)

try:
    import pytesseract
except ImportError:  # pragma: no cover
    pytesseract = None

if pytesseract is not None and shutil.which("tesseract") is None:
    # PATH may not include the install dir in every process/session even
    # though the binary is present (verified locally: winget installs it
    # here but doesn't broadcast the PATH update to already-running shells).
    _default_win_install = Path(r"C:\Program Files\Tesseract-OCR\tesseract.exe")
    if _default_win_install.exists():
        pytesseract.pytesseract.tesseract_cmd = str(_default_win_install)


@dataclass
class Control:
    x: float
    y: float
    radius: float
    code: str | None = None  # OCR'd control code, e.g. "46" from a "7-46" label; None if unread


@dataclass
class CourseResult:
    controls: list[Control] = field(default_factory=list)
    start: tuple[float, float] | None = None
    finish: tuple[float, float] | None = None
    legs: list[LineString] = field(default_factory=list)
    ink_mask: np.ndarray | None = None  # exposed so segmentation can exclude these pixels from terrain


def build_course_ink_mask(img: np.ndarray, valid_mask: np.ndarray) -> np.ndarray:
    """Broad warm red/maroon/magenta mask -- covers every course-overprint
    color seen across the test photos (see module docstring). Deliberately
    wide; shape-based classification downstream is what separates real
    course marks from other warm-colored map content."""
    hsv = cv2.cvtColor(img, cv2.COLOR_BGR2HSV)
    h, s, v = cv2.split(hsv)
    red_low = (h <= 15) | (h >= 150)
    warm_enough = (s > 70) & (v > 40) & (v < 235)
    mask = red_low & warm_enough & valid_mask
    mask_u8 = (mask.astype(np.uint8)) * 255
    mask_u8 = cv2.morphologyEx(mask_u8, cv2.MORPH_CLOSE, np.ones((3, 3), np.uint8))
    return mask_u8 > 0


def detect_controls(mask: np.ndarray, img_shape: tuple[int, int], param2: int) -> list[Control]:
    """Control circles: hollow rings of a fairly consistent diameter across
    the whole map (the diameter is a fixed mm value at the map's print
    scale). HoughCircles on the ink mask directly, rather than
    contour/circularity analysis, since a thin ring's own outer contour is a
    circle already and Hough is the more standard/robust tool for that.

    No single param2 generalizes across photos (see config.HOUGH_PARAM2) --
    checked directly against map0.jpg and map2.jpg's manually-counted
    baselines, and a threshold that's about right for one is off by 2-4x on
    the other, because one photo's course-ink mask is simply noisier than
    the other's. A modal-radius consistency filter (real controls share one
    print diameter) was tried as a way to avoid needing a per-file value,
    but measured worse in practice -- Hough's radius estimates on these
    photos are noisy enough (std ~13px on map0.jpg, no sharp peak) that the
    mode just picks a different noisy cluster, not a clean signal. Simple
    near-duplicate suppression is kept below since that part did help.
    Caller is expected to pass config.HOUGH_PARAM2.get(filename,
    HOUGH_PARAM2_DEFAULT).
    """
    h, w = img_shape[:2]
    diag = np.hypot(h, w)
    mask_u8 = (mask.astype(np.uint8)) * 255
    # The sheet's own printed border is a long, strong, near-circular-at-corners
    # edge that would otherwise compete with real control circles.
    mx, my = int(w * 0.015), int(h * 0.015)
    mask_u8[:my, :] = 0
    mask_u8[-my:, :] = 0
    mask_u8[:, :mx] = 0
    mask_u8[:, -mx:] = 0
    blurred = cv2.GaussianBlur(mask_u8, (5, 5), 0)
    circles = cv2.HoughCircles(
        blurred, cv2.HOUGH_GRADIENT, dp=1.2, minDist=diag * 0.02,
        param1=60, param2=param2, minRadius=int(diag * 0.006), maxRadius=int(diag * 0.02),
    )
    if circles is None:
        return []

    out = []
    for x, y, r in circles[0]:
        # cv2.HoughCircles returns candidates ordered by accumulator
        # strength (best first); skip anything too close to an already-kept
        # circle instead of double-detecting the same ring.
        if any(np.hypot(x - c.x, y - c.y) < r for c in out):
            continue
        if _ring_ink_coverage(mask, x, y, r) < RING_COVERAGE_MIN_FRACTION:
            continue
        out.append(Control(x=float(x), y=float(y), radius=float(r)))
    return out


def _ring_ink_coverage(mask: np.ndarray, x: float, y: float, r: float, band: int = 3, samples: int = 144) -> float:
    """Fraction of a candidate circle's own circumference that actually has
    ink under it, sampled every 2.5 degrees and tolerant of the Hough
    radius estimate being off by a few px (band). A real printed control
    ring is one continuous stroke, so a correct detection's circle traces
    ink almost the whole way around; a HoughCircles false positive fit
    through scattered non-ring ink -- clutter, or (checked directly on
    map0.jpg) the round "0" glyphs in a nearby control's own printed code
    label -- only grazes ink along a fraction of its circumference. Checked
    directly against every accepted/rejected circle on all four
    IN_SCOPE_FILES: real rings scored >=0.82, every confirmed false
    positive (including two that a same-file median-radius comparison
    couldn't distinguish, since their radii sat inside the real cluster's
    own range) scored <=0.72 -- see config.RING_COVERAGE_MIN_FRACTION."""
    h, w = mask.shape
    hits = 0
    for i in range(samples):
        theta = 2 * np.pi * i / samples
        for dr in range(-band, band + 1):
            px = int(x + (r + dr) * np.cos(theta))
            py = int(y + (r + dr) * np.sin(theta))
            if 0 <= px < w and 0 <= py < h and mask[py, px]:
                hits += 1
                break
    return hits / samples


def _dedupe_finish(controls: list[Control], dist_tol_frac: float = 0.5) -> tuple[list[Control], tuple[float, float] | None]:
    """Finish is drawn as two concentric circles. Hough will report both as
    separate detections with close centers and different radii -- merge the
    closest such pair into a single finish point and drop both from the
    control list."""
    for i, a in enumerate(controls):
        for j, b in enumerate(controls):
            if i >= j:
                continue
            dist = np.hypot(a.x - b.x, a.y - b.y)
            if dist < max(a.radius, b.radius) * dist_tol_frac and abs(a.radius - b.radius) > 1.5:
                finish = ((a.x + b.x) / 2.0, (a.y + b.y) / 2.0)
                remaining = [c for k, c in enumerate(controls) if k not in (i, j)]
                return remaining, finish
    return controls, None


def detect_start_triangle(mask: np.ndarray, exclude_disks: list[Control]) -> tuple[float, float] | None:
    """Start is a triangle (not a circle) in the same ink. Searches connected
    components of the ink mask, excluding pixels already claimed by a
    detected control/finish circle, for one that approximates a 3-vertex
    polygon with roughly equal sides *and* a size consistent with this
    file's own detected control-circle scale (config.START_TRIANGLE_*).

    This reliably returns None on every in-scope photo as of this writing --
    that's the honest, checked-directly result, not a bug to chase. Five
    separate approaches were tried to make this actually find the real
    triangle, and all five failed for concrete, verified reasons:

    1. The original version of this function (no size gate, equilateral
       threshold 0.6) does return *a* 3-vertex contour, but cropping the
       photo at its reported coordinates shows plain terrain, no triangle
       ink -- it's noise. The "triangles" it accepts are 30-220px^2 (a real
       triangle at these files' print scale should be ~2000-4000px^2, per
       START_TRIANGLE_AREA_RATIO_RANGE's comment) with visibly skewed sides
       (e.g. 44.7/25.7/23.6px) that the old 0.6 threshold didn't reject.
    2. Recalibrating the warm-hue mask tighter, using HSV sampled directly
       from this file's own confirmed control-circle ink as the reference:
       no improvement. Sampled real ring ink and known false positives
       (a printed road, a contour-line tangle, a sponsor logo) side by side
       -- their HSV percentiles are statistically indistinguishable (e.g.
       H median 7 vs 8-17, S median 116-140 vs 68-127 across all three,
       heavily overlapping). These elements are printed in genuinely similar
       red/brown ink families; no threshold separates them by color.
    3. Local illumination flattening (dividing by a heavily-blurred copy per
       channel, the document-scanner "remove shading/vignette" trick) before
       re-sampling: same result, no separation opens up. This rules out
       "it's just a bad white balance" -- the confusion is in the printed
       ink colors themselves, not photo lighting quality.
    4. Comparing local stroke thickness (2x distance-transform) of confirmed
       control-ring ink against the rest of the mask: medians match (4px
       both) -- course lines and contour/road lines print at the same
       weight on these maps, no thickness signal either.
    5. Rotation-invariant template matching (a synthetic hollow triangle at
       the expected size, tested at 15-degree steps): found a strong,
       120-degree-symmetric peak -- genuinely promising until the matched
       location was cropped. It was a straight course leg crossing a
       V-shaped kink in an out-of-bounds/area-boundary line, not a triangle;
       every top candidate was the same kind of incidental line crossing.

    Manually scanning the full rectified photo (including a saturation-
    boosted crop and every config.LEGEND_EXCLUDE_BOXES region, in case the
    triangle was hidden under a hand-excluded logo box) didn't turn up an
    obvious separate triangle mark either. Conclusion: on these photos, at
    this resolution, the real start triangle is not reliably distinguishable
    from ordinary map linework by color, shape, size, thickness, or template
    matching -- reaching it would need a fundamentally different approach
    (e.g. a trained symbol classifier), which is Phase-3 ML-segmentation
    territory, not a Phase-0 CV heuristic. Don't re-attempt any of the five
    approaches above without new evidence for why they'd work differently
    next time (see plan's guidance on this pattern, also applied to the
    abandoned OCR-declutter and radius-consistency-filter attempts).
    """
    work = mask.copy()
    for c in exclude_disks:
        cv2.circle(work, (int(c.x), int(c.y)), int(c.radius * 1.4), 0, thickness=-1)
    mask_u8 = (work.astype(np.uint8)) * 255
    contours, _ = cv2.findContours(mask_u8, cv2.RETR_EXTERNAL, cv2.CHAIN_APPROX_SIMPLE)

    if not exclude_disks:
        return None  # no controls detected to calibrate the expected triangle size against

    median_circle_area = float(np.median([np.pi * c.radius ** 2 for c in exclude_disks]))
    min_area = START_TRIANGLE_AREA_RATIO_RANGE[0] * median_circle_area
    max_area = START_TRIANGLE_AREA_RATIO_RANGE[1] * median_circle_area

    best = None
    best_score = -1.0
    for c in contours:
        area = cv2.contourArea(c)
        if area < min_area or area > max_area:
            continue
        peri = cv2.arcLength(c, True)
        approx = cv2.approxPolyDP(c, 0.04 * peri, True)
        if len(approx) != 3:
            continue
        pts = approx.reshape(3, 2).astype(float)
        sides = [np.hypot(*(pts[i] - pts[(i + 1) % 3])) for i in range(3)]
        equilateral = 1.0 - (np.std(sides) / max(np.mean(sides), 1e-6))
        if equilateral > best_score:
            best_score = equilateral
            best = pts.mean(axis=0)
    if best is None or best_score < START_TRIANGLE_MIN_EQUILATERAL_SCORE:
        return None
    return float(best[0]), float(best[1])


def detect_legs(mask: np.ndarray, exclude_disks: list[Control], start: tuple[float, float] | None) -> list[LineString]:
    """Connecting lines between controls: what's left of the ink mask once
    circles/triangle are carved out is thin line segments -- detect them the
    same way as segmentation.py's path lines (short HoughLinesP segments,
    draft quality, not merged into single polylines per leg)."""
    work = mask.copy()
    for c in exclude_disks:
        cv2.circle(work, (int(c.x), int(c.y)), int(c.radius * 1.4), 0, thickness=-1)
    if start is not None:
        cv2.circle(work, (int(start[0]), int(start[1])), int(mask.shape[0] * 0.02), 0, thickness=-1)
    # Same border-frame exclusion as detect_controls, and for the same
    # reason: checked directly (course_detection false positives, see plan)
    # that the sheet's own printed border ends up in the broad course-ink
    # mask on all four in-scope photos and Hough was tracing long "leg"
    # segments along it, running the full width/height of the sheet.
    h, w = mask.shape[:2]
    my, mx = int(h * 0.015), int(w * 0.015)
    work[:my, :] = False
    work[-my:, :] = False
    work[:, :mx] = False
    work[:, -mx:] = False
    mask_u8 = (work.astype(np.uint8)) * 255
    edges = cv2.Canny(mask_u8, 40, 120)
    # minLineLength scaled to image size and set well above a control-code
    # digit's stroke length (short text like "7-46" next to each circle
    # otherwise floods this with thousands of tiny spurious segments --
    # verified empirically on map0.jpg: minLineLength=12 gives ~5600
    # segments, mostly digit strokes, vs ~90 at this scaled threshold).
    min_len = mask.shape[0] * 0.04
    lines = cv2.HoughLinesP(edges, 1, np.pi / 180, threshold=20, minLineLength=min_len, maxLineGap=15)
    if lines is None:
        return []
    out = []
    for seg in lines:
        x1, y1, x2, y2 = np.asarray(seg).reshape(-1).astype(float)
        if np.hypot(x2 - x1, y2 - y1) < min_len:
            continue
        out.append(LineString([(x1, y1), (x2, y2)]))
    return out


_CODE_PATTERN = re.compile(r"\d{1,2}-\d{1,3}")


def ocr_control_code(img: np.ndarray, control: Control, ink_mask: np.ndarray) -> str | None:
    """OCR the control-code text printed next to a control circle (e.g. the
    "46" in a "7-46" label). Best-effort: requires the Tesseract-OCR binary
    installed separately (pytesseract only binds to it) -- returns None
    rather than raising if it's unavailable or nothing legible is found,
    since this must never fabricate a code that wasn't actually read.

    OCRs the course-ink *mask* within a generous window around the circle,
    not the raw photo -- course setters place the code label in whichever
    of the 4 quadrants around the circle has free space (not a fixed
    offset), and OCRing the raw grayscale crop picks up contour lines and
    vegetation-boundary ink as false text. Restricting to ink-mask pixels
    (with the circle itself blanked out as a filled disk, since Hough's
    radius estimate is noisy enough that a thin ring mask can miss part of
    the real printed circle) cuts that noise down enough for --psm 11
    (sparse text) to isolate the digit-dash label. Checked directly against
    map0.jpg's golden-path photo: this raised the legible-code yield from
    1/17 controls (old fixed-offset raw-grayscale crop) to ~9/17 -- real
    photos are noisy enough that recovering roughly half is the realistic
    best-effort bar here, not full coverage.
    """
    if pytesseract is None:
        return None
    r = control.radius
    h, w = img.shape[:2]
    search = int(r * 6)
    x0, x1 = max(0, int(control.x - search)), min(w, int(control.x + search))
    y0, y1 = max(0, int(control.y - search)), min(h, int(control.y + search))
    roi_mask = ink_mask[y0:y1, x0:x1].copy()
    if roi_mask.size == 0:
        return None
    cv2.circle(roi_mask, (int(control.x - x0), int(control.y - y0)), int(r * 1.6), False, thickness=-1)
    binary = np.where(roi_mask, 0, 255).astype(np.uint8)
    binary = cv2.resize(binary, None, fx=3, fy=3, interpolation=cv2.INTER_NEAREST)
    binary = cv2.medianBlur(binary, 3)
    try:
        data = pytesseract.image_to_data(
            binary, config="--psm 11 -c tessedit_char_whitelist=0123456789-",
            output_type=pytesseract.Output.DICT,
        )
    except Exception:
        return None
    best_code, best_conf = None, -1.0
    for text, conf in zip(data["text"], data["conf"]):
        match = _CODE_PATTERN.search(text.strip())
        if match and float(conf) > best_conf:
            best_code, best_conf = match.group(0), float(conf)
    return best_code


def detect_course(
    img: np.ndarray, valid_mask: np.ndarray, run_ocr: bool = True, source_filename: str | None = None,
) -> CourseResult:
    param2 = HOUGH_PARAM2.get(source_filename, HOUGH_PARAM2_DEFAULT)
    mask = build_course_ink_mask(img, valid_mask)
    controls = detect_controls(mask, img.shape, param2)
    controls, finish = _dedupe_finish(controls)
    start = detect_start_triangle(mask, controls)
    legs = detect_legs(mask, controls, start)

    if run_ocr:
        for c in controls:
            c.code = ocr_control_code(img, c, mask)

    return CourseResult(controls=controls, start=start, finish=finish, legs=legs, ink_mask=mask)
