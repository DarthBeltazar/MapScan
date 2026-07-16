"""Shared configuration for the map-photo -> vector pipeline (Phase 0 prototype).

Scope: classic forest-ISOM orienteering map photos only (see plan). Sprint/
rogaine/alpine map photos in testData are intentionally not covered here.
"""

from __future__ import annotations

# Closed set of ISOM terrain classes this pipeline recognizes -- matches the
# taxonomy fixed in prompt.txt (forest / clearing / thicket / paths / water /
# rock / out-of-competition). This is the enum the future cost-grid step will
# key its running-speed lookup table on, so it must stay closed (no ad-hoc
# extra classes).
TERRAIN_CLASSES = (
    "forest",         # лес (проходимый)
    "clearing",       # поляна / открытое пространство
    "thicket",        # чаща / труднопроходимая растительность
    "path",           # тропы / дороги
    "water",          # вода
    "rock",           # скалы / камни (текстура, не цвет)
    "out_of_bounds",  # зона вне соревнования
)

# Working resolution cap for all CV steps (max side, px). Source photos are
# ~9248x6936; full-res coordinates are recoverable via the scale factor
# returned by preprocessing.preprocess_image.
WORKING_MAX_SIDE = 3000

# Per-file fractional exclude boxes (x0, y0, x1, y1), each in [0, 1] relative
# to the *rectified working-resolution* image, covering legend tables /
# titles / sponsor logos that would otherwise contaminate terrain-color
# clustering and course-ink detection. Deliberately manual for this Phase-0
# prototype (see plan) -- calibrated by visually inspecting
# output/<name>_qa.png and iterating, not by automatic layout detection.
LEGEND_EXCLUDE_BOXES: dict[str, list[tuple[float, float, float, float]]] = {
    "map0.jpg": [
        (0.0, 0.0, 1.0, 0.09),      # title/date/scale strip
        (0.64, 0.0, 1.0, 0.38),     # control-description grid + class column
        (0.44, 0.10, 0.61, 0.28),   # "Нескучный сад" inset map
        (0.245, 0.108, 0.36, 0.28),  # Clever/Moscompass logos
        (0.07, 0.31, 0.16, 0.445),  # round club logo
        (0.0275, 0.466, 0.13, 0.552),  # "AA 90" organizer box
        (0.345, 0.402, 0.435, 0.574),  # "Массовые старты" logo
        (0.07, 0.61, 0.195, 0.775),  # "Искатель Москва" logo
        (0.4375, 0.323, 0.645, 0.402),  # "Читайте нас в сети" box
        (0.645, 0.38, 0.81, 0.43),  # "Результаты On-line" box
        (0.0075, 0.854, 0.08, 0.965),  # bottom-left sponsor logo
        (0.915, 0.861, 0.995, 0.954),  # bottom-right federation logo
    ],
    # Coarser pass (title block + legend only) -- not individually calibrated
    # against every sponsor logo like map0.jpg above, see plan's note on this
    # being an iterative, empirically-tuned config. IMPORTANT: the title
    # block's *position* varies per photo, not just its content -- it isn't
    # reliably "top of frame". find_paper_quad's corner-ordering doesn't
    # preserve reading orientation (see preprocessing.py), so the same
    # physical title strip lands on a different edge of the rectified image
    # depending on which corner that run happened to call "top-left". A flat
    # 6% top/bottom band was verified to miss most of a large bold-red title
    # (e.g. "КВАРК-2022") on map4.jpg/map6.jpg entirely, which then got
    # misread as course overprint ink (both colors are warm/saturated) --
    # every box below was positioned by looking at the actual rectified
    # image for that file, not assumed from a shared template.
    # Recalibrated after fixing find_paper_quad's corner-cropping bug (it was
    # silently cutting off the corner carrying the title on map2.jpg/map4.jpg
    # -- these boxes were positioned against the *old*, wrongly-cropped
    # framing and stopped matching once that got fixed).
    "map2.jpg": [
        (0.0, 0.0, 1.0, 0.05),
        (0.72, 0.0, 1.0, 0.32),  # title block + KBAPK logo, top-right here
        (0.0, 0.72, 0.42, 1.0),  # control-description grid, bottom-left here
    ],
    "map4.jpg": [
        (0.0, 0.0, 1.0, 0.04),
        (0.65, 0.0, 0.97, 0.32),  # title block, top-right here
        (0.0, 0.75, 0.22, 1.0),   # bottom-left sponsor/club logo cluster
    ],
    "map6.jpg": [
        (0.83, 0.0, 1.0, 0.55),  # title block sits along the right edge here
        (0.0, 0.70, 0.38, 1.0),  # bottom-left sponsor/club logo cluster
    ],
}

# Per-file HoughCircles accumulator threshold for control-circle detection
# (course_detection.detect_controls). No single value generalizes: checked
# directly against map0.jpg and map2.jpg's manually-counted baselines (see
# MANUAL_KP_COUNTS below), and the same threshold that's about right for one
# is off by 2-4x on the other -- map2.jpg's course ink mask is simply
# cleaner/less noisy than map0.jpg's, so it needs a *lower* accumulator
# threshold to find enough real circles while map0.jpg needs a *higher* one
# to reject noise. HOUGH_PARAM2_DEFAULT is an unverified middle-ground guess
# for files with no manual baseline to calibrate against (map4.jpg,
# map6.jpg) -- expect it to be off in the same way, not a substitute for
# actually calibrating those two.
HOUGH_PARAM2_DEFAULT = 38
HOUGH_PARAM2: dict[str, int] = {
    "map0.jpg": 46,  # -> 17 found vs manual count 18
    "map2.jpg": 48,  # -> 9 found vs manual count 9 (recalibrated after fixing find_paper_quad's crop bug)
}

# Files this Phase-0 round targets (classic forest ISOM). Everything else in
# testData (sprint/ISSprOM, alpine, rogaine topo-base maps) is explicitly out
# of scope this round -- see plan's Context section for why.
IN_SCOPE_FILES = ("map0.jpg", "map2.jpg", "map4.jpg", "map6.jpg")

# Manually counted control-point baselines from the printed control-description
# grids on each map photo (one observer, not audited ground truth -- see plan's
# testing section). Used only as a golden-path regression invariant.
MANUAL_KP_COUNTS = {
    "map0.jpg": 18,
    "map2.jpg": 9,
}
