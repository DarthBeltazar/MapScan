# Phase 0 → Phase 1 handoff

Phase 0 (this repo, Python) is done: it proved the chain "photo → rectify →
segment terrain/course → cost-grid → least-cost path" works end to end on
real photographed maps. Phase 1 is a **different codebase** (Flutter client +
Rust via `flutter_rust_bridge`, per `prompt.txt`'s fixed stack) — nothing here
gets ported line-for-line. This doc is a condensed manifest of what to carry
forward conceptually, what calibration is photo-specific and must not be
copied as-is, and what already failed so it isn't re-attempted blind. Full
narrative/rationale for all of it lives in `CLAUDE.md` and the module
docstrings it points at; this doc only indexes it for someone starting Phase 1
who won't read the whole history first.

## What Phase 0 actually proved (and didn't)

- Proved: perspective rectification → HSV/shape-based terrain + course
  segmentation → per-pixel cost raster → `route_through_array` shortest path
  is a workable pipeline shape, on 4 real in-scope photos
  (`config.IN_SCOPE_FILES`: map0/2/4/6, classic forest ISOM only).
- Did **not** prove: control-sequencing/ordered course routing (Phase 0 has
  no course graph — `run_pipeline.py`'s demo route is a single connectivity
  leg, not a real course), manual correction (Phase 1's mandatory first
  requirement per `prompt.txt` — nothing here implements it), robust
  number/control-code OCR (best-effort only, ~0-9/17 controls depending on
  file), or start-triangle detection (reliably fails, see below) — all
  explicitly Phase 1+ scope.
- No labeled ground truth exists anywhere in this repo. `MANUAL_KP_COUNTS` is
  one observer's eyeball count, not audited data — don't treat Phase 0's
  "passes" as validated accuracy.

## Algorithms worth reimplementing conceptually in Rust/OpenCV-FFI

Each of these is a *validated approach*, not code to transliterate — the
Python versions are draft-quality prototypes per `prompt.txt`'s rules.

- **Paper-quad detection**: largest-contour + `approxPolyDP` on the contour's
  **convex hull**, not the raw contour (raw contour lets a shadow/glare notch
  produce a "clean" quad that actually clips a real corner). Fallback to
  `minAreaRect` of the hull if the quad covers <85% of hull area (safe-loose
  over precise-wrong). `preprocessing.find_paper_quad`.
- **Reading-orientation correction must run before any OCR/near-vertical
  logic**, and corner-ordering alone can't determine which corner is
  physically "top" — needs either a hand-calibrated table (Phase 0's stopgap)
  or a real detector; Phase 0 tried automatic rotation-scoring via OCR
  confidence across all 4 rotations and it wasn't reliable enough to trust.
  `preprocessing.correct_reading_rotation`.
- **White balance via percentile white-patch** using the photo's own
  brightest pixels (paper margin) as the white reference — real, measured
  per-photo color casts (~30pt off neutral on blue channel in one case) push
  map browns into the course-ink hue range if skipped. `preprocessing.white_balance`.
- **Vegetation k-means clustering must run on a median-blurred copy**, not raw
  per-pixel HSV — raw-pixel clustering fragments into thousands of
  single-pixel islands that an area filter then silently discards (~80%+ area
  loss). `segmentation.py`'s k-means step.
- **`make_valid` on pixel-contour polygons can return `GeometryCollection`**,
  not just `Polygon`/`MultiPolygon` — code that only handles the latter two
  silently drops geometry (this was a real bug: whole lake polygons
  vanishing). Whatever polygon library Rust uses needs the equivalent
  defensive unwrap. `segmentation._mask_to_polygons`.
- **Course ink color is not a fixed hue** across events (red/maroon/magenta
  all seen) — classify course elements by *shape* (Hough circles/triangles/
  line segments) against a broad warm-hue mask, not by exact color match.
  `course_detection.build_course_ink_mask`.
- **Ring-ink-coverage post-filter beats radius-based filtering** for
  rejecting false-positive Hough circles — checked directly that real vs.
  false detections overlap in radius but separate cleanly (>=0.82 vs <=0.72)
  on ink coverage along the circumference. `course_detection._ring_ink_coverage`,
  `config.RING_COVERAGE_MIN_FRACTION`.
- **Cost-grid draw order matters on polygon overlap**: draw stricter/more
  expensive terrain classes last so they win overlaps, don't let draw order
  be arbitrary. `cost_grid._AREA_DRAW_ORDER`.
- **Off-mask and off-polygon pixels should get a finite penalty, not a
  barrier/inf** — segmentation gaps and rectification slop near start/end
  points are artifacts, not real terrain; making them impassable breaks
  pathfinding on real photos. `config.DEFAULT_TERRAIN_COST`,
  `config.OUTSIDE_VALID_MASK_COST`.

## Dead ends — don't re-attempt without new evidence

All of these were tried in Phase 0 and measured no better (or worse) than the
alternative actually used. Full accounts are in the relevant docstrings.

- **Radius-consistency filtering** for Hough circle false-positive rejection
  (either as a pre-filter before threshold selection, or as a global filter) —
  Hough's radius estimates on these photos are too noisy for a mode/consensus
  to mean anything. Ring-ink-coverage (above) is what actually worked.
- **Start-triangle detection** — reliably returns `None` on all 4 in-scope
  photos. Five distinct approaches failed for five distinct, verified
  reasons (unguarded shape search, tighter HSV recalibration, illumination
  flattening, stroke-thickness comparison, rotation-invariant template
  matching — the last one found a strong false peak that was a leg-line/
  boundary-line kink, not a triangle). `course_detection.detect_start_triangle`'s
  docstring has the full account. Likely needs a trained symbol classifier
  (Phase 3 ML territory), not another classical-CV heuristic.
- **OCR layout decluttering** by dropping large connected components
  (probable leg lines) before Tesseract, and component-clustering by
  bounding-box size for per-label text isolation — neither improved yield
  over doing nothing when a label sits close to crossing leg lines.
- **Automatic rotation-scoring** (OCR confidence across all 4 rotations) to
  replace the hand-calibrated `PAGE_ROTATION_K` table — not reliable enough
  to trust.

## Calibration data: hand-tuned, not portable as literal numbers

Every constant in `python_prototype/pipeline/config.py` below the terrain-class enum is
per-photo, hand-calibrated by looking at that specific file's rendered QA
output — `PAGE_ROTATION_K`, `LEGEND_EXCLUDE_BOXES`, `HOUGH_PARAM2`,
`RING_COVERAGE_MIN_FRACTION`, `START_TRIANGLE_AREA_RATIO_RANGE`. None of it
generalizes to a new photo, let alone a new event's map. **This is the
concrete reason Phase 1's manual-correction UI is not a nice-to-have** — it's
the only way a real user's photo (never seen by this repo) gets a workable
result instead of silently wrong thresholds. Don't hardcode a fixed per-file
table in the Flutter/Rust app; if anything, expose the equivalent knobs
(rotation, exclude regions, circle-detection sensitivity) through the
correction UI itself, or derive them from the manual corrections a user makes.

The one constant that *is* meant to generalize as an ordering (not as
calibrated speeds) is `TERRAIN_COST` — path < clearing < forest < rock <
marsh < thicket, water/out-of-bounds strongly avoided. No field data exists
to calibrate real running speeds; keep treating this as qualitative ISOM
convention until real data shows up.

## Test-data scope reminder

`python_prototype/testData/` has 9 real photos across 3 map genres; only `map0/2/4/6.jpg`
(classic forest ISOM) are in scope for the taxonomy this pipeline (and
`TERRAIN_CLASSES`) was built around. `map1.jpg` (alpine), `map3/5.jpg`
(ISSprOM sprint, buildings-dominated), `map7/8.jpg` (1:15000-17500 rogaine
topo-base) need a different taxonomy entirely — extending to them is a scope
decision for whoever picks that up, not an assumption to carry into Phase 1
silently.

## What Phase 1 needs that doesn't exist anywhere yet

- Manual correction UI (add/move/delete control, recolor terrain area) — the
  hard requirement from `prompt.txt`, and the actual reason automatic
  detection accuracy above is treated as "best-effort" throughout Phase 0.
- Control-sequencing / ordered course graph — Phase 0's `CourseResult.controls`
  is an unordered list; there is no leg-ordering logic to port.
- Dijkstra/A* in Rust — Phase 0 used `skimage.graph.route_through_array`
  as a stand-in per `prompt.txt`'s note that Python routing is prototype-only;
  Rust needs its own implementation, not a wrapped/ported call into Python.
- Purple-layer Hough-circle autodetection tuned against real MVP photos,
  from scratch — Phase 0's per-file `HOUGH_PARAM2` table doesn't transfer
  (see above).
