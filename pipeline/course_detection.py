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

from pipeline.config import HOUGH_PARAM2, HOUGH_PARAM2_DEFAULT

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
        out.append(Control(x=float(x), y=float(y), radius=float(r)))
    return out


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
    """Start is a triangle (not a circle) in the same ink. Search connected
    components of the ink mask, excluding pixels already claimed by a
    detected control/finish circle, for one that approximates a 3-vertex
    polygon with roughly equal sides."""
    work = mask.copy()
    for c in exclude_disks:
        cv2.circle(work, (int(c.x), int(c.y)), int(c.radius * 1.4), 0, thickness=-1)
    mask_u8 = (work.astype(np.uint8)) * 255
    contours, _ = cv2.findContours(mask_u8, cv2.RETR_EXTERNAL, cv2.CHAIN_APPROX_SIMPLE)
    best = None
    best_score = -1.0
    for c in contours:
        area = cv2.contourArea(c)
        if area < 30:
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
    if best is None or best_score < 0.6:
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
