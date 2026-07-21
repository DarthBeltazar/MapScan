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

# Per-file reading-orientation correction: number of 90-degree
# counterclockwise rotations (see preprocessing.correct_reading_rotation)
# applied to the rectified image so the title text comes out horizontal and
# upright. find_paper_quad's corner-ordering has no way to know which corner
# of the paper is physically "top" -- checked directly by cropping each
# file's title block and reading it at all 4 rotations: map0.jpg needs none,
# map2.jpg needs 1 (comes out 90 degrees clockwise of reading orientation),
# map4.jpg/map6.jpg both need 3 (90 degrees *counterclockwise* -- i.e. the
# opposite direction from map2.jpg, not a shared global offset). All of
# LEGEND_EXCLUDE_BOXES/HOUGH_PARAM2 below are calibrated against the
# post-rotation image. Unlisted filenames default to 0 (no rotation).
PAGE_ROTATION_K: dict[str, int] = {
    "map2.jpg": 1,
    "map4.jpg": 3,
    "map6.jpg": 3,
}

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
        # Recalibrated after adding preprocessing.correct_reading_rotation
        # (config.PAGE_ROTATION_K): this file now gets rotated 90 degrees
        # clockwise post-rectify so its title text reads upright, which
        # moves every box below relative to the old (unrotated) layout --
        # re-derived by inspecting the corrected image, not rotated by
        # formula from the old boxes.
        (0.0, 0.0, 1.0, 0.05),    # thin strip right along the paper's top
        # edge -- excludes noise from the sheet's own printed border/table
        # edge, same purpose as the equivalent strip on other files.
        (0.0, 0.0, 0.40, 0.18),   # "2023" + round "КВАРК" club badge,
        # top-left here.
        (0.58, 0.48, 1.0, 0.91),  # title block + control-description grid
        # (the "D2 3,7 km" table -- its cell borders and corner circle/
        # triangle glyphs are exactly the kind of shapes HoughCircles and
        # detect_start_triangle look for, so this needs excluding same as
        # the title text does), bottom-right here.
    ],
    "map4.jpg": [
        # Recalibrated after adding preprocessing.correct_reading_rotation
        # (config.PAGE_ROTATION_K): this file now gets rotated 270 degrees
        # counterclockwise post-rectify so its title text reads upright,
        # which moves every fixed-position box below relative to the old
        # (unrotated) layout -- these are *not* the same boxes rotated by
        # formula, they were re-derived by inspecting the corrected image.
        (0.12, 0.0, 0.25, 0.05),  # "75 ФИЗТЕХ" badge, top-left here (the
        # other top-left logos -- МФТИ atom, round "КВАРК" club badge, blue
        # running-figure icon -- are blue/black, not warm-hued, so they
        # don't show up in build_course_ink_mask at all and don't need
        # excluding).
        (0.55, 0.48, 1.0, 0.80),  # title block ("M1:7500 H2,5m КВАРК-2022"),
        # bottom-right here.
    ],
    "map6.jpg": [
        # Recalibrated after adding preprocessing.correct_reading_rotation
        # (config.PAGE_ROTATION_K): this file now gets rotated 270 degrees
        # counterclockwise post-rectify so its title text reads upright,
        # which moves every box below relative to the old (unrotated)
        # layout -- re-derived by inspecting the corrected image, not
        # rotated by formula from the old boxes.
        (0.03, 0.04, 0.17, 0.12),  # "75 ЛЕТ ФИЗТЕХ" badge, top-left here
        (0.63, 0.48, 1.0, 0.78),   # title block ("M1:7500 H2,5m КВАРК-2022"),
        # bottom-right here.
    ],
}

# Per-file HoughCircles accumulator threshold for control-circle detection
# (course_detection.detect_controls). No single value generalizes: checked
# directly against each file's manually-counted baseline (see
# MANUAL_KP_COUNTS below), and the same threshold that's about right for one
# is off by 2-4x on another -- each photo's course-ink mask has its own
# amount of noise (road-symbol tan, shoreline curves, and -- before the
# LEGEND_EXCLUDE_BOXES fix -- title-text glyphs all sit in the same warm-hue
# band as real course ink, and HoughCircles will fit a "circle" to any of
# them if the threshold is too permissive). map4.jpg/map6.jpg were
# originally uncalibrated (falling back to HOUGH_PARAM2_DEFAULT=38) and it
# showed: map4.jpg returned 32 "controls" at that threshold for a true count
# of ~17, and map6.jpg's false circle on the "КВАРК" title text was only
# caught after fixing that file's LEGEND_EXCLUDE_BOXES (the old box didn't
# actually cover the title -- see above). Radius consistency across a file's
# accepted detections is a useful *sanity check* while calibrating (real
# controls print at one fixed diameter, so a clean run has a tight radius
# spread and a noisy one has outliers up to 2x the rest) but is deliberately
# not used as an automatic filter -- see detect_controls' docstring for why
# that was tried and abandoned as a global filter.
HOUGH_PARAM2_DEFAULT = 38
HOUGH_PARAM2: dict[str, int] = {
    "map0.jpg": 46,  # -> 17 found vs manual count 18
    "map2.jpg": 44,  # -> 9 found vs manual count 9; recalibrated again (was
    # 48) after adding PAGE_ROTATION_K, same reason as map4.jpg below -- also
    # needed the top-border-strip exclude box restored (see
    # LEGEND_EXCLUDE_BOXES) since without it, no single threshold separated
    # a real 9th control from a border-noise false positive of similar
    # accumulator strength.
    "map4.jpg": 46,  # -> 17 found vs manual count ~17 (see MANUAL_KP_COUNTS);
    # recalibrated (was 54) after adding PAGE_ROTATION_K -- circle shapes are
    # rotation-invariant so this shouldn't have needed to change much, and it
    # didn't (still a clean tight radius spread at this value), but the
    # image-edge margin detect_controls masks out is a fraction of width/
    # height, which swap on a 90-degree rotation, so the exact pixel margins
    # differ slightly.
    "map6.jpg": 42,  # -> 9 found vs manual count ~9 (see MANUAL_KP_COUNTS); stable
    # across param2 in [38, 44], i.e. not a knife-edge fit to one value.
}

# Files this Phase-0 round targets (classic forest ISOM). Everything else in
# testData (sprint/ISSprOM, alpine, rogaine topo-base maps) is explicitly out
# of scope this round -- see plan's Context section for why.
IN_SCOPE_FILES = ("map0.jpg", "map2.jpg", "map4.jpg", "map6.jpg")

# Manually counted control-point baselines (one observer, not audited ground
# truth -- see plan's testing section). Used only as a golden-path
# regression invariant. map0.jpg/map2.jpg were counted from each photo's own
# printed control-description grid; map4.jpg/map6.jpg have no such grid
# visible in frame, so those two were counted by tiling the raw photo into
# overlapping crops and counting printed circle+code labels by eye -- a
# weaker form of "manual" than the other two, more prone to miscounting a
# label as two controls or missing one hidden behind line clutter.
MANUAL_KP_COUNTS = {
    "map0.jpg": 18,
    "map2.jpg": 9,
    "map4.jpg": 17,
    "map6.jpg": 9,
}

# --- Phase-0 cost-grid / path-finding (prompt.txt phase 0: prove
# "segmentation -> cost-grid -> path" works end to end) ---

# Relative traversal cost per unit distance, by terrain class (lower = a
# runner crosses it faster). These are a *qualitative* ISOM-convention
# ordering -- path fastest, then clearing, then forest, then rock, then
# thicket, with water/out-of-bounds treated as strongly avoided -- not a
# measured running-speed model. No field data exists to calibrate real speeds
# against (see plan's note on not fabricating validated numbers), so these
# values only need to get the *ordering* right for the pathfinder to prefer
# runnable terrain; don't read them as calibrated m/s figures.
TERRAIN_COST = {
    "path": 0.8,
    "clearing": 1.0,
    "forest": 1.5,
    "rock": 2.5,
    "thicket": 4.0,
    "water": 20.0,
    "out_of_bounds": 50.0,
}

# Cost for pixels not covered by any detected terrain polygon (gaps between
# polygons, k-means clusters discarded as noise, etc). Treated as neutral
# ("clearing"-like) rather than a barrier, since these gaps are a segmentation
# artifact, not real terrain.
DEFAULT_TERRAIN_COST = TERRAIN_COST["clearing"]

# Cost for pixels outside the valid (paper) mask entirely. Kept finite (not
# inf) so a start/end point landing just outside the valid mask due to
# rectification slop doesn't make path-finding fail outright -- but high
# enough that the router only ever crosses it when there's truly no other way.
OUTSIDE_VALID_MASK_COST = TERRAIN_COST["out_of_bounds"]

# Segmentation's path LineStrings are zero-width centerlines; this is how
# many pixels wide to draw them into the cost grid so they actually register
# as cheap terrain rather than being lost between grid cells.
PATH_COST_LINE_WIDTH_PX = 3

# Start-triangle acceptance gate (course_detection.detect_start_triangle).
# Per IOF spec the triangle's side length equals the control circle's
# diameter, so its area should be ~0.55x a control circle's area
# (area_triangle = (sqrt(3)/4)*side^2 vs area_circle = pi*(side/2)^2). This
# range is a wide margin around that ratio to tolerate this pipeline's own
# noisy per-file radius estimates (see HOUGH_PARAM2's comment) -- not itself
# a claim that 0.55 is exact. Checked directly against map0.jpg: every
# false-positive "triangle" the unguarded shape search was accepting (noise
# blobs 30-220px^2, vs a ~2000-4000px^2 real triangle at that file's circle
# scale) falls far below this range and gets correctly rejected.
START_TRIANGLE_AREA_RATIO_RANGE = (0.25, 1.3)

# Equilateral-ness threshold (1.0 - side_length_std/mean; see
# detect_start_triangle) for a candidate contour to be accepted as the start
# triangle. Raised from an earlier 0.6 after checking directly: on map0.jpg
# the false positives the old threshold accepted scored 0.65-0.78 despite
# being visibly non-equilateral by eye (e.g. sides 44.7/25.7/23.6px) once
# their contours were cropped and inspected -- 0.6 wasn't discriminating
# shape at all, just letting the size gate above do all the work.
START_TRIANGLE_MIN_EQUILATERAL_SCORE = 0.85
