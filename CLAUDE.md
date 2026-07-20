# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project context

This repo is **Phase 0** of a larger, long-lived project: a cross-platform mobile app for orienteering
that lets a runner photograph a paper orienteering map and get an offline route between controls. The
full project spec (roles, architecture, fixed tech stack, phase plan) lives in `prompt.txt` (Russian) —
read it before doing any non-trivial work here, since it defines constraints this repo must not violate.

Key facts from `prompt.txt` that matter when working in *this* repo:

- **This repo is the Python CV/path-finding prototype only** (`prompt.txt` phase 0: "prove segmentation →
  cost-grid → path works"). Production mobile code is Flutter (Dart) + Rust (via `flutter_rust_bridge`);
  Python here is prototyping only, not something that gets ported/shipped as-is. Don't blur the two worlds
  without an explicit reason.
- The **fixed stack** for the eventual product (Flutter/Rust/OpenCV-FFI/TFLite/ML Kit/SQLite) is decided —
  don't propose alternatives unless explicitly asked, and don't design this Python code as if it were the
  production implementation.
- The internal map format is a **GeoJSON-like structure** (polygons for terrain classes + points for
  controls). `.omap`/`.ocd` (OpenOrienteeringMapper/OCAD) are a separate, unbuilt *input* bypass path for
  already-digitized maps — not something this pipeline generates.
- **Don't skip ahead by phase.** Phase 0 stops at "photo → vector map"; cost-grid and path-finding
  (Dijkstra/A* → later Fast Marching Method) are the *next* phase and are not implemented in this repo yet.
  Don't pull later-phase work (ML segmentation, live GPS, Fast Marching) into a phase-0 task "while you're
  at it."
- **No labeled ground truth exists** for the sample photos in `testData/` — don't fabricate accuracy
  numbers or synthetic "ground truth" to make something look validated. Quality is judged by the QA
  overlay PNGs (visual) and a couple of manually-counted control baselines (`config.MANUAL_KP_COUNTS`),
  which are explicitly one-observer counts, not audited data.
- Before writing non-trivial code, state the plan and key assumptions first.

## Scope: which test photos are "in scope"

`testData/` has 9 real photographed orienteering maps, but they're **three different map genres** and only
one genre is targeted by the current pipeline:

- **In scope** (`pipeline.config.IN_SCOPE_FILES`): `map0.jpg`, `map2.jpg`, `map4.jpg`, `map6.jpg` — classic
  forest ISOM maps, which is what `config.TERRAIN_CLASSES` (лес/поляна/чаща/тропы/вода/скалы/зона вне
  соревнования) was designed around.
- **Out of scope, don't tune for these**: `map1.jpg` (alpine), `map3.jpg`/`map5.jpg` (ISSprOM sprint —
  buildings dominate, wrong taxonomy), `map7.jpg`/`map8.jpg` (1:15000–1:17500 rogaine topo-base maps —
  also wrong taxonomy). If asked to extend the pipeline to these, that's a scope change to raise, not
  assume.
- All four in-scope files have a manually-counted control baseline now (`config.MANUAL_KP_COUNTS`).
  `map0.jpg`/`map2.jpg` are still the stronger "golden path" pair — counted from each photo's own printed
  control-description grid; `map4.jpg`/`map6.jpg`'s counts were counted by tiling the raw photo and eyeballing
  printed circle+code labels (no such grid visible in frame for those two), a weaker form of "manual" more
  prone to miscounting — see `MANUAL_KP_COUNTS`'s docstring in `config.py`.

## Commands

Environment: Python 3.14, venv at `.venv/` (already created). No `requirements.txt` yet — the dependency
set actually installed is:

```
opencv-python-headless numpy shapely scikit-image pytesseract matplotlib pytest Pillow
```

Windows (PowerShell/Git Bash), always use the venv's own interpreter rather than relying on activation:

```
.venv/Scripts/python.exe -m pip install opencv-python-headless numpy shapely scikit-image pytesseract matplotlib pytest Pillow
```

Run the pipeline on one photo (writes `output/<name>.geojson` + `output/<name>_qa.png`):

```
.venv/Scripts/python.exe scripts/run_pipeline.py testData/map0.jpg
.venv/Scripts/python.exe scripts/run_pipeline.py testData/map0.jpg --no-ocr --out-dir output
```

`--no-ocr` skips control-code OCR — useful to save the ~0.3s/control OCR cost when iterating on something
unrelated. The **Tesseract-OCR binary** (`pytesseract` only binds to it) is installed via winget
(`UB-Mannheim.TesseractOCR`) at `C:\Program Files\Tesseract-OCR\tesseract.exe`;
`pipeline/course_detection.py` points `pytesseract` at that path directly as a fallback in case a given
process's `PATH` hasn't picked up the winget install (verified locally: it doesn't propagate to
already-running shells). If the binary is genuinely missing, OCR calls still fail gracefully and return
`None` rather than crash. Even with the binary present, treat control codes as **best-effort, not
reliable** — `ocr_control_code` OCRs the course-ink mask (not the raw photo) in a window around each
circle to cut out contour-line/vegetation noise. Current yield (all four in-scope photos, checked
directly): map0.jpg ~9/17, map6.jpg ~6/9, map4.jpg ~3/17, map2.jpg ~0/9.

The single biggest lever here turned out to be *global page orientation*, not anything OCR-specific: three
of the four in-scope photos (`config.PAGE_ROTATION_K`) come out of `rectify()` rotated 90 degrees off
reading orientation, because `find_paper_quad`'s corner-ordering has no way to know which corner of the
paper is physically "top" -- confirmed directly by reading each file's title block at all 4 multiples of
90 degrees (see `preprocessing.correct_reading_rotation`'s docstring). Before that rotation was corrected,
Tesseract couldn't read text on those files *at all*, no matter how clean the underlying ink was --
verified directly, a manually-rotated crop of map4.jpg's course-code text OCR'd perfectly at the correct
angle and failed completely at the photo's original (uncorrected) angle. Correcting it is what took
map4.jpg/map6.jpg from ~0/N to their current yield.

What's left after that fix is a second, different problem: even with text right-side-up, Tesseract's own
layout analysis (`--psm 11`) still gets confused when a label sits close to several crossing leg lines --
verified directly on map2.jpg, whose labels are upright and legible to the eye in the ink-mask crop but
still OCR to garbage, because the surrounding line clutter gets segmented as competing "text" regions.
Decluttering by dropping large connected components (probable leg lines) before OCR did not fix this on
its own. Reliably solving it would need per-label text-region isolation tight enough to exclude the
clutter entirely, which was attempted (component-clustering by bounding-box size) and measured no better
than doing nothing -- same story as the abandoned radius-consistency filter in `detect_controls`'s
docstring: don't re-attempt that specific approach without new evidence for *why* it would work better
next time.

Run the whole test suite / a single test:

```
.venv/Scripts/python.exe -m pytest tests/ -q
.venv/Scripts/python.exe -m pytest tests/test_geometry.py::test_dedupe_finish_merges_close_concentric_pair -q
```

There's no lint/format tooling configured in this repo.

## Architecture: the pipeline

`scripts/run_pipeline.py:run()` wires the stages together; each stage is its own module under `pipeline/`:

```
preprocessing.preprocess_image(path)
  → segmentation.default_valid_mask()  (legend/title exclusion from config.LEGEND_EXCLUDE_BOXES)
  → course_detection.detect_course()   (needs source_filename for config.HOUGH_PARAM2 lookup)
  → segmentation.segment_terrain()     (masked to exclude course ink so it isn't read as terrain fill)
  → vectorize.build_feature_collection()  → output/<name>.geojson
  → visualize.render_qa_overlay()         → output/<name>_qa.png
```

**`preprocessing.py`** — photo → rectified, color-normalized working-resolution image
(`config.WORKING_MAX_SIDE` = 3000px longer side). In order:
1. `load_image_exif_safe` — loads with `cv2.IMREAD_IGNORE_ORIENTATION`. This is deliberate, not an
   oversight: checked directly against these photos, their EXIF orientation tags are wrong (rotate the
   *already-correct* raw pixels into a bad orientation), and `cv2.imread` applies EXIF automatically as of
   OpenCV 4.5+ unless told not to.
2. `find_paper_quad` — finds the photographed sheet's 4 corners via largest-contour + `approxPolyDP` on
   the contour's **convex hull** (not the raw contour — a shadow/glare notch in the raw contour was
   verified to make `approxPolyDP` return a "clean" quad that actually cuts off a real corner of the
   paper). If the resulting quad covers <85% of the hull's area, that's treated as the same failure mode
   and it falls back to `cv2.minAreaRect` of the hull instead (safe-but-loose over precise-but-wrong).
3. `rectify` — perspective warp to that quad.
4. `correct_reading_rotation` — rotates the rectified image by a per-file multiple of 90 degrees
   (`config.PAGE_ROTATION_K`) so it's in reading orientation (title text horizontal, upright).
   `find_paper_quad`'s corner-ordering picks a consistent (tl, tr, br, bl) labeling for whatever quad it
   finds, but nothing in the photo tells it which physical corner is "top" — checked directly by cropping
   each in-scope file's title block and reading it at all 4 rotations: `map0.jpg` needs none, `map2.jpg`
   comes out 90 degrees clockwise of reading orientation, `map4.jpg`/`map6.jpg` both come out 90 degrees
   the *other* way. Not just cosmetic: this must run before step 5 (near-vertical-only) and before any
   OCR, and is a hardcoded per-file lookup, not automatic detection — an automatic rotation-scoring
   detector was tried (OCR confidence across all 4 rotations, both whole-image and on the largest
   near-white margin region) and wasn't reliable enough to trust, so this is hand-calibrated the same way
   `LEGEND_EXCLUDE_BOXES`/`HOUGH_PARAM2` already are.
5. `white_balance` — percentile white-patch balance using the photo's own brightest pixels (the paper
   margin) as the white reference. Not optional/cosmetic: these photos carry a real, per-photo color cast
   (checked directly — one photo's "white" paper background measured BGR (185, 196, 217), ~30 points off
   neutral on blue) that otherwise pushes ordinary map browns into the course-ink hue range.
6. `detect_magnetic_north_lines` — best-effort only. Deliberately does *not* search for line families at
   arbitrary angles (a version of this that did was more likely to lock onto a spurious diagonal pattern
   and mis-rotate an already-correct image than to find the real lines) — it only looks near-vertical,
   which is only a safe assumption because step 4 already fixed reading orientation. Frequently returns
   nothing on real photos (thin printed lines get constantly broken up by other map content); nothing
   downstream depends on it succeeding.

**`segmentation.py`** — rectified image → draft terrain polygons (`config.TERRAIN_CLASSES`, one shapely
polygon list per class) + path `LineString`s. Order of operations matters: water (blue hue) and
out-of-bounds (purple hue) are pulled out first, then rock (texture/edge-density on light background, not
color — ISOM rock is dot/hatch pattern), then paths (dark thin linework, short unmerged Hough segments,
draft quality on purpose), and whatever's left is k-means clustered in HSV (on a **median-blurred** copy —
clustering on raw per-pixel HSV was verified to fragment into thousands of single-pixel islands that then
get discarded by the area filter, silently losing ~80%+ of the vegetation area) into forest/clearing/
thicket by a hand-tuned hue/value heuristic (`_classify_vegetation_cluster`).

`_mask_to_polygons` matters more than it looks: `shapely.validation.make_valid` on a self-touching raw
pixel-contour polygon frequently returns a `GeometryCollection`, not a `Polygon`/`MultiPolygon` — code that
only handles the latter two silently drops the geometry. This was a real bug here (whole lake polygons
vanishing); the fix (extracting polygonal parts out of any returned geom type) is why this function looks
more defensive than a "just call make_valid" version would.

**`course_detection.py`** — rectified image → controls/start/finish/connector-legs. The printed course
color is **not a fixed hue across maps** (checked against all 9 test photos: red, dark maroon, and
pink/magenta all show up depending on the event) — `build_course_ink_mask` casts a broad warm-hue net and
everything downstream classifies by *shape* (circle via `HoughCircles`, triangle via 3-vertex
`approxPolyDP`, thin lines via Hough segments), not by exact color.

`detect_controls`'s Hough accumulator threshold (`param2`) does **not generalize across photos** — one
photo's course-ink mask is measurably noisier than another's, so a threshold tuned against one file's
manually-counted baseline can be off 2-4x on another. It's a per-file override
(`config.HOUGH_PARAM2[filename]`, falling back to `HOUGH_PARAM2_DEFAULT`); all four in-scope files are now
calibrated (`HOUGH_PARAM2_DEFAULT` itself is still an unverified guess — don't assume it's right for a
file that isn't in the dict). A "smarter" radius-consistency filter (real controls share one print
diameter) was tried as a way to avoid per-file tuning and measured *worse* in practice (Hough's radius
estimates on these photos are too noisy for the mode to mean anything) — don't re-attempt that without new
evidence. Radius spread across a file's *accepted* detections is still a useful manual sanity check while
calibrating, though (see `HOUGH_PARAM2`'s comment in `config.py`) — just not a filter the code applies
automatically.

**`vectorize.py`** — assembles segmentation + course results into one GeoJSON `FeatureCollection`, in the
rectified image's own local pixel coordinates (no real-world geo-referencing — out of scope by design, see
`prompt.txt`). Y is flipped (`height - y`) so the output is genuinely GeoJSON-convention (Y up), not
image-convention (Y down) mislabeled as GeoJSON. Carries a non-standard top-level `properties` block
(source photo, scale, detected line spacing) for `visualize.py` and, eventually, a Flutter renderer to
overlay the vector data back onto the photo without distortion.

**`config.py`** is where all the per-file empirical calibration lives (`PAGE_ROTATION_K`,
`LEGEND_EXCLUDE_BOXES`, `HOUGH_PARAM2`, `MANUAL_KP_COUNTS`) — when a photo's rectification or exclusion
zones change (e.g. after a `preprocessing.py` fix), these need re-checking; they were hand-positioned by
looking at that specific file's rectified/QA output, not derived from a formula. `PAGE_ROTATION_K` in
particular changes the *geometry* every other per-file constant is expressed in (rotating swaps width/
height and moves every fixed-position box) — if it ever changes for a file, `LEGEND_EXCLUDE_BOXES` and
`HOUGH_PARAM2` for that file need re-deriving from scratch against the newly-corrected image, not adjusted
by formula from the old ones.

## Testing

Three tiers, deliberately not one (see `tests/*.py` docstrings for the reasoning):

- `test_geometry.py` — synthetic inputs, exact-match assertions on deterministic geometry helpers
  (simplify, `make_valid` handling, mask→polygon, Hough-family shape classification).
- `test_pipeline_golden.py` — real photos (`map0.jpg`, `map2.jpg`), but *invariant* checks (aspect ratio
  sanity, control count within a tolerance band of `MANUAL_KP_COUNTS`, segmented area within a plausible
  fraction of the valid mask) rather than exact-match, since there's no pixel-level ground truth.
- `test_smoke.py` — parametrized over `config.IN_SCOPE_FILES`, just asserts the full pipeline runs without
  raising and produces non-empty output.

When you change anything in `preprocessing.py` or `segmentation.py`'s masking, re-run the full pipeline on
all of `IN_SCOPE_FILES` and eyeball the `_qa.png` outputs before trusting the numbers — several real bugs
here (a silently-dropped `GeometryCollection`, a wrongly-cropped paper quad) passed a casual glance at
counts and only showed up on the rendered overlay.
