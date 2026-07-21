# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project context

This repo covers a larger, long-lived project: a cross-platform mobile app for orienteering that lets a
runner photograph a paper orienteering map and get an offline route between controls. The full project
spec (roles, architecture, fixed tech stack, phase plan) lives in `prompt.txt` (Russian) — read it before
doing any non-trivial work here, since it defines constraints this repo must not violate.

The repo root now holds multiple sub-projects side by side:

- **`python_prototype/`** — the Phase 0 Python CV/path-finding prototype, complete and frozen. Everything
  below this point in the doc describes that subtree. `PHASE0_HANDOFF.md` (repo root) is the condensed
  manifest of what to carry forward into Phase 1 (Flutter/Rust), what calibration is photo-specific and
  non-portable, and what already failed there so it isn't re-attempted blind — read it before starting
  Phase 1 work.
- **`app/`** — Phase 1: Flutter client + Rust core crate (`app/rust/`), wired together via
  `flutter_rust_bridge` (scaffolded with `flutter_rust_bridge_codegen create`, not a port of
  `python_prototype/`). Toolchain on this machine: Flutter SDK at `C:\flutter` (stable channel, not
  inside the repo), Rust via rustup, Android SDK/NDK at `%LOCALAPPDATA%\Android\Sdk`, `flutter_rust_bridge_codegen`
  via `cargo install`. `flutter doctor` is clean; both `flutter build windows` and `flutter build apk --debug`
  are verified working from this checkout.
  - **Known gotcha, checked directly**: this Flutter version's template ships Gradle 9.1.0/AGP 9.0.1, but
    `flutter_rust_bridge` 2.12.0's vendored `cargokit` Gradle plugin
    (`app/rust_builder/cargokit/gradle/plugin.gradle`) still calls the now-removed `Project.exec()` API,
    which fails Android builds with `Could not find method exec() ... on project of type
    org.gradle.api.Project`. Fixed locally by injecting `ExecOperations` into the task
    (`@Inject abstract ExecOperations getExecOperations()`) instead of calling `project.exec {}` — the
    Gradle-recommended replacement, not a version downgrade. Also had to bump
    `app/rust_builder/android/build.gradle`'s `compileSdkVersion` from the plugin's default of 33 to 36 to
    match `android/app/build.gradle.kts`'s `flutter.compileSdkVersion`, since its transitive `androidx`
    deps require compileSdk ≥34. **Both edits are hand-patches to files `flutter_rust_bridge_codegen`
    generates/vendors** — if the project is ever regenerated or `rust_builder/cargokit` is refreshed from a
    newer frb release, re-check whether upstream has fixed Gradle 9 compatibility before reapplying (crates.io
    still listed 2.12.0 as the newest *stable* release when this was hit; a 2.13.0 beta existed but wasn't used).

  ### Rust CV pipeline (in progress) and the OpenCV toolchain

  `app/rust/src/api/preprocessing.rs` has the first reimplemented pipeline stage: `rectify_photo`
  (decode -> downscale -> paper-quad detection -> perspective warp -> percentile white-patch balance ->
  best-effort magnetic-north-line detection -> PNG-encode), a conceptual Rust port of the Python
  prototype's `preprocessing.find_paper_quad`/`preprocessing.rectify`/`preprocessing.white_balance`/
  `preprocessing.detect_magnetic_north_lines` — not line-for-line. Reading-orientation correction is
  deliberately still not reimplemented: the Python version's rotation fix is a hardcoded per-file table
  (`config.PAGE_ROTATION_K`) that `PHASE0_HANDOFF.md` explicitly says not to carry into Phase 1 as-is (it
  doesn't generalize to a photo this repo hasn't seen — that knob belongs in the manual-correction UI
  instead, not a fixed table). Verified directly against all 4 in-scope real photos via
  `app/rust/tests/rectify_photo.rs` (`cargo test --test rectify_photo`) — paper quad found and rectified on
  all of them; white balance was additionally checked numerically (not just via the test's
  dimension/PNG-validity assertions) by sampling the rectified map4.jpg's paper-margin pixels before/after
  — the documented ~30pt-off-neutral cast on that file's blue channel collapses to within ~2 units across
  channels after correction. Magnetic-north-line detection behaves the same as the Python version on these
  4 files (checked directly, not just "it compiles"): map6.jpg finds 4 lines at a consistent ~176px
  spacing, map0.jpg/map4.jpg only clear the >=3-lines-with-vertical-span bar for 2 candidates each (so
  spacing comes back `None`, matching the Python version's documented "frequently finds nothing" behavior),
  and map2.jpg finds 3 widely-spaced lines that are very likely a false-positive family rather than the
  real grid — expected per the Python docstring's own caveat that this is best-effort and nothing
  downstream depends on it succeeding, not something to chase further right now. `RectifyResult` grew
  `mn_line_xs`/`mn_line_spacing_px` fields for this (regenerated via `flutter_rust_bridge_codegen generate`
  from `app/`, surfaced in `main.dart`'s demo screen) — re-run that generate step and rebuild whenever a
  `#[frb]`-visible struct's fields change, same as after this addition.

  `app/rust/src/api/segmentation.rs` and `app/rust/src/api/course_detection.rs` port the Python prototype's
  `segmentation.py`/`course_detection.py` (terrain polygons + path lines; course-ink controls/start/finish/
  legs) — conceptual ports, not line-for-line, wired together with `preprocessing.rs` by
  `app/rust/src/api/analyze.rs`'s `analyze_map` (the new frb-exposed entry point; `rectify_photo` still
  exists standalone). Key differences from the Python version, deliberate not oversights:
  - **No polygon-validity/geometry crate.** Python's `_mask_to_polygons` handles shapely returning
    `GeometryCollection` from `make_valid` on self-touching pixel-contour rings (see that function's
    docstring). The Rust port's `Polygon` is a plain simplified point ring (`approxPolyDP` in place of
    shapely's `simplify`, same Douglas-Peucker algorithm underneath) with no validity concept to repair in
    the first place — a deliberate simplification consistent with "draft quality, human cleans up
    downstream" (see `segmentation.rs`'s module doc), not a gap to backfill reflexively. If a later stage
    (the cost-grid rasterizer) needs true polygon validity, that's the point to add a real geometry crate.
  - **OCR is not ported.** `course_detection.ocr_control_code` needs a separately-installed Tesseract
    binary — bundling/requiring a native OCR engine on Android is a distinct infrastructure decision, not
    something to pull in incidentally while porting shape detection. `Control.code` is always `null`.
  - **Legend-exclude boxes and the HoughCircles `param2` threshold are caller-supplied parameters
    (`ExcludeBox` list, `houghParam2: f64`), not a per-file table baked into the crate** — same
    `PHASE0_HANDOFF.md` guidance as `PAGE_ROTATION_K` above: Python's `config.LEGEND_EXCLUDE_BOXES`/
    `HOUGH_PARAM2` don't generalize to a photo this repo hasn't seen, so those knobs belong behind a future
    manual-correction/sensitivity-tuning UI, not hardcoded. `app/rust/tests/analyze.rs` reuses the Python
    prototype's already-calibrated per-file values for its 4 in-scope test photos *for that test only* (to
    exercise the port against a known-good calibration) — `main.dart`'s demo screen deliberately does NOT
    do this, calling `analyzeMap` with an empty exclude-box list and the Python prototype's
    `HOUGH_PARAM2_DEFAULT` (38.0, itself documented there as an unverified guess) regardless of which photo
    the user picks, since that's the honest uncalibrated result a real new photo gets today.
  - **A naming-collision trap, worth remembering if this happens again**: flutter_rust_bridge indexes types
    by short name across the whole `crate::api` tree it's told to scan (`rust_input: crate::api` in
    `flutter_rust_bridge.yaml`), not by fully-qualified path — a private `Segment` struct inside
    `preprocessing.rs` (magnetic-north-line segments) silently collided with the public, frb-exposed
    `geometry::Segment` (course legs / path lines) until renamed to `MnSegment`; codegen only warned
    ("multiple objects with same key ... randomly pick one") rather than erroring. Similarly,
    `flutter_rust_bridge_codegen generate` does NOT respect `pub(crate)` as "don't bridge this" — it still
    tried to surface the internal `Preprocessed` handoff struct (which carries a raw `opencv::core::Mat`)
    as an opaque Dart class purely because it's reachable under `crate::api`, which then failed to compile
    (`cannot find type Mat in this scope` in the generated glue) until marked
    `#[flutter_rust_bridge::frb(ignore)]` explicitly. Relatedly, the individual algorithm-step functions in
    `segmentation.rs`/`course_detection.rs` (`segment_terrain`, `detect_controls`, `detect_start_triangle`,
    `detect_legs`, `detect_course`) must stay `pub(crate)`, not `pub` — making any of them fully `pub` makes
    frb try to bridge that function too as a standalone Dart-callable entry point, which broke the same way
    for `segment_terrain` (it takes a raw `&Mat` parameter, which isn't a supported bridged parameter type
    here). Only `analyze_map`/`rectify_photo` and the plain-data struct types are meant to be `pub`.
  - **Verified directly against all 4 in-scope real photos**, not just "it compiles" (`app/rust/tests/analyze.rs`,
    `cargo test --test analyze`): control counts land within a wide tolerance of the Python prototype's
    manually-counted baselines (map0.jpg 14 found/18 manual, map2.jpg 10/9, map4.jpg 10/17, map6.jpg 8/9),
    segmented terrain area is a plausible (neither ~0 nor ~100%) fraction of the total image on every file,
    and all 7 terrain classes always produce (possibly empty) polygon lists. Additionally spot-checked
    visually (rendered a QA overlay PNG the same way the Python prototype's `visualize.py` does, via a
    throwaway example binary, then deleted it): control-circle detections sit precisely on real printed
    circles on map6.jpg. That same run's `detect_start_triangle` **did** report a triangle (Python's version
    reliably reports `None` on every in-scope photo) — cropping the reported location confirmed it's the
    purple out-of-bounds diagonal-hatch pattern crossing itself, i.e. exactly the same class of false
    positive Python's own docstring documents at length (a line crossing a kink mistaken for a triangle),
    not a porting bug — this detector is expected to be unreliable on these photos in *either* language, see
    `course_detection.rs`'s `detect_start_triangle` doc comment and the Python original's docstring before
    trying to "fix" it.
  - **`out_of_bounds` detection quality on real photos, checked directly on map2.jpg**: real violet/purple
    ISOM out-of-bounds cross-hatching genuinely exists on this file (confirmed by cropping the raw,
    unrectified photo at 3 locations found by scanning for the same hue range `out_of_bounds_mask` uses) —
    "not detected at all" was the initial impression from the demo app's overlay, but that's not quite
    right. Cropping individual detected `out_of_bounds` polygons showed a mix: at least one (the largest,
    ~1700px²) sits exactly on real hatching; at least two much smaller ones are false positives on printed
    maroon control-code/control-description-grid *digits* ("0", "9") that happen to fall in the same
    135-170 hue band `out_of_bounds_mask` casts. This is the same "real course ink and printed text are
    statistically indistinguishable in HSV" problem `detect_start_triangle`'s docstring already documents
    for course ink specifically — it hadn't been checked directly against `out_of_bounds` before. Separately,
    even where the real hatching *is* found, it comes back as many small disconnected polygon fragments (a
    diagonal cross-hatch pattern doesn't bridge into one blob under `_mask_to_polygons`'/`mask_to_polygons`'s
    small 5x5 `MORPH_CLOSE` kernel), not one clean zone shape. Net effect: real signal exists in the output,
    but is currently outnumbered by false positives and fragmentation badly enough that a user looking at
    the raw overlay reasonably reads it as "not detected" — this is a real, now-confirmed weakness (likely
    shared with the Python original, which has no test coverage of `out_of_bounds` specifically either), not
    something to silently accept as fine.
  - **Tightening `out_of_bounds_mask`'s hue/saturation range was tried next and measured worse, not fixed**
    (see `segmentation.rs`'s `out_of_bounds_mask` doc comment for the full account) — don't re-attempt
    without new evidence. Sampled-HSV percentiles showed real hatching and the false-positive digits have
    nearly identical saturation distributions at every threshold checked (no separating gap), and this was
    confirmed empirically, not just from the percentiles: raising the saturation floor from 40 to 70 (to
    match `build_course_ink_mask`'s own threshold) verified end-to-end against all 4 in-scope photos
    destroyed real signal *more* than false positives — map2.jpg's confirmed-real ~1727px² hatching polygon
    shrank to a 121px² remnant, and map0.jpg's out-of-bounds detection (4 polygons) dropped to zero
    entirely. Raising the hue floor from 135 to 150 was worse still: zero out-of-bounds polygons on
    map0.jpg/map4.jpg/map6.jpg. Same "statistically indistinguishable in HSV" conclusion
    `detect_start_triangle` already reached for course-ink-vs-triangle confusion, now independently confirmed
    for this mask too. Reverted cleanly to the original 135-170/40-255/40-255 range.
  - **Shape-based discrimination, tried next, worked** (`segmentation.rs`'s `out_of_bounds_polygons` — see
    its doc comment for the full account): real ISOM cross-hatching, once closed into a blob by the same
    `MORPH_CLOSE` step every class's polygon extraction uses, has *several* small enclosed holes (the
    diamond gaps between hatch lines — checked directly, map2.jpg's two confirmed real zones had 5, 5, and
    7 holes each), while isolated printed digits have 0-1 (a "0"'s single counter, or none for a "9"). A
    `RETR_CCOMP` hole count (`MIN_HATCH_HOLES = 3`) replaces the plain area-only extraction for this class
    only. Verified end-to-end against all 4 in-scope photos: map0.jpg correctly stays at 0 (no real hatching
    on that file either); map2.jpg went from 29 polygons (mostly noise) to exactly 1, its confirmed-real
    zone; map6.jpg went from 22 to 2, both cropped and confirmed real, zero false positives remaining.
    map4.jpg initially went from 31 to 5, of which only 2 were real — the other 3 were a *different* false
    positive (merged multi-character title/scale text like "H2,5m"/"7500", where adjacent glyphs bridge into
    one blob and collectively accumulate enough holes), not the isolated-digit case this filter targets.
    Covered by 3 new synthetic unit tests in `segmentation.rs` (`cargo test --lib`: a multi-hole lattice is
    kept, a single-hole ring and a no-hole solid blob are both rejected) in addition to the real-photo
    verification above.
  - **The map4.jpg title-text residual led to a separate, now-fixed bug**: `tests/analyze.rs`'s calibrated
    `legend_boxes` for map4.jpg/map6.jpg were copied from the Python prototype's
    `config.LEGEND_EXCLUDE_BOXES`, calibrated against Python's *rotated* image (`PAGE_ROTATION_K` rotates
    both files 270 degrees before that config was derived) — since this Rust pipeline deliberately doesn't
    rotate (see `preprocessing.rs`), those boxes landed in the wrong place here. Confirmed directly by
    rendering `rectify_photo`'s plain output with a fractional grid overlay for both files: in this
    pipeline's actual (unrotated) frame the title block is top-right and the "75 ЛЕТ ФИЗТЕХ"/КВАРК/МФТИ logo
    cluster is bottom-left for both files — roughly 180 degrees from where Python's boxes assume they are.
    Re-derived both files' boxes directly against the Rust-rectified images (same visual-inspection method
    the Python prototype's own calibration used) and updated `tests/analyze.rs`:
    map4.jpg → `eb(0.49, 0.0, 0.97, 0.43)` (title) + `eb(0.0, 0.60, 0.30, 0.96)` (logos), map6.jpg →
    `eb(0.49, 0.0, 0.78, 0.33)` + `eb(0.0, 0.68, 0.29, 0.97)`. Effect, verified via `cargo test --test
    analyze`: map4.jpg's out_of_bounds false positives dropped from 3 to 0 (2 real polygons, 0 false, checked
    directly by bounding-box comparison against the confirmed-real locations) — the previously-unexcluded
    title text was exactly what those merged-digit holes were coming from. Control detection improved
    substantially too, as a side effect of less title-text clutter feeding the Hough circle search:
    map4.jpg 10/17 found→manual → 16/17, map6.jpg 8/9 → 9/9 (exact), and map6.jpg's previously-observed
    spurious `detect_start_triangle` false positive (see above) no longer fires. This fix only touches the
    test harness's calibration constants, not production code — `main.dart`'s demo path still deliberately
    uses no legend boxes at all (see `PHASE0_HANDOFF.md`'s guidance against hardcoding per-file tables in
    the shipped app).
  - The demo app's layer-filter UI (isolate/hide a class, hide small polygons) remains useful for whatever
    residual noise a given photo still has.

  `app/rust/src/api/cost_grid.rs` and `app/rust/src/api/pathfinding.rs` port the Python prototype's
  `cost_grid.py`/`pathfinding.py` (terrain/path rasterization into a per-pixel traversal-cost grid; a demo
  least-cost route between two points), wired into `analyze_map`'s orchestration right after segmentation +
  course detection, mirroring `run_pipeline.py`'s `_pick_demo_route_endpoints` logic (start -> nearest
  control, else the first two detected controls, else no route) verbatim in
  `analyze.rs::pick_demo_route_endpoints`. `AnalyzeResult` grew `route: Option<RouteResult>` and
  `route_terrain_breakdown: Vec<TerrainFraction>` fields for this, drawn as a yellow line by `main.dart`'s
  overlay painter. `TERRAIN_COST` (path/clearing/forest/rock/marsh/thicket/water/out_of_bounds ordering) is
  hardcoded in `cost_grid.rs`, unlike `LEGEND_EXCLUDE_BOXES`/`HOUGH_PARAM2` — this is the one piece of the
  Python prototype's `config.py` explicitly meant to generalize as a qualitative ISOM ordering, not
  per-photo calibration (see `PHASE0_HANDOFF.md`), so hardcoding it here isn't the same mistake.
  - **Pathfinding is a from-scratch Rust implementation, not a wrapped port.** The Python prototype uses
    `skimage.graph.route_through_array` (`MCP_Geometric`) precisely because `prompt.txt` calls Python
    routing logic prototype-only and names that function for Phase 0; `PHASE0_HANDOFF.md` explicitly says
    Rust needs its own implementation. `pathfinding.rs` is an 8-connected Dijkstra (`BinaryHeap` + lazy
    deletion) over the flat cost grid, using `MCP_Geometric`'s own documented per-step weighting verbatim
    (a step of Euclidean length `d` costs `d * (cost[p1] + cost[p2]) / 2`) so that `RouteResult`'s
    independent `recomputed_cost` resum (same self-consistency check the Python version does) actually
    lines up with the Dijkstra-reported `cost`.
  - **Verified on synthetic grids first** (13 `#[cfg(test)] mod tests` unit tests inside `cost_grid.rs`/
    `pathfinding.rs`, `cargo test --lib`), each a direct Rust port of the corresponding case in
    `python_prototype/tests/test_cost_grid.py`/`test_pathfinding.py` (draw-order/overlap winner, path drawn
    cheap over area fill, valid-mask exclusion, route prefers a cheap corridor over an expensive wall,
    direct route on a uniform grid, out-of-bounds endpoint clamping, hand-computed geometric-cost check) —
    same reasoning as those Python tests: exact-match assertions are possible here because this is
    deterministic rasterization/graph-search, not a CV heuristic that needs a real photo.
  - **Then verified end-to-end against all 4 real in-scope photos** (`app/rust/tests/analyze.rs`, extended,
    `cargo test --test analyze`): a demo route is found on every file (each one has either a start+control
    or >=2 controls), stays within the image bounds, `cost`/`recomputed_cost` agree to within 1% on the real
    (non-square, non-synthetic) grid, and terrain-breakdown fractions sum to 1. Route cost on the full
    ~3000px-wide working-resolution grid added ~1.8s/photo to the test in an unoptimized debug build (23s
    total for 4 photos, up from 16s pre-route) — not fast enough to worry about optimizing yet, but worth
    knowing if a future photo count/resolution increase makes debug-build test time annoying; the shipped
    app would run this in a release build regardless. The resulting routes are qualitatively sane, not just
    "didn't crash": map0.jpg/map2.jpg/map4.jpg's routes are dominated by detected `path` terrain
    (60-67% of route length) — the router correctly preferring the cheapest terrain class where a real path
    was found — while map6.jpg's route (using the start->nearest-control branch, on that file's spurious
    start-triangle detection, see above) is mostly `forest`/`default_gap`, consistent with not having a
    detected path to follow between those two particular points.

  `app/rust/src/api/vectorize.rs` ports the Python prototype's `vectorize.py` (segmentation + course +
  route results -> a single GeoJSON-shaped document, the project's internal map format per `prompt.txt` --
  see that Python module's docstring for why this is *not* real-world geo-referenced, and why `.omap`/`.ocd`
  are a separate unbuilt input path this doesn't touch). `build_geojson` returns a plain `String` (not a
  typed struct) — GeoJSON's per-feature `properties` are inherently heterogeneous (a control's `code`, a
  route's `terrain_breakdown` map), which fits `serde_json::Value` naturally and a single frb struct
  awkwardly, and a string is also what a caller actually wants (write straight to a `.geojson` file, same as
  the Python prototype's `run_pipeline.py`, no separate parsing step needed on the Dart side). This is the
  first `serde_json` usage in the crate — added as a plain dependency (`Cargo.toml`), no native/C toolchain
  implications unlike `opencv`.
  - `AnalyzeResult` grew a `geojson: String` field (built automatically inside `analyze_map`, alongside a
    new `source_filename: Option<String>` parameter feeding the `properties.source_photo` field) and
    `RectifyResult`/the internal `Preprocessed` type grew `scale_to_original: f32` (previously computed in
    `preprocessing::downscale` but discarded — needed for `properties.scale_to_original_photo`, matching the
    Python prototype's `PreprocessResult.scale_to_original`; multiply a working-resolution pixel coordinate
    by `1.0 / scale_to_original` to recover its position in the original full-resolution photo).
  - **Verified with 6 synthetic unit tests** (`cargo test --lib`, exact-match on deterministic
    transforms/serialization, same reasoning as `cost_grid.rs`/`pathfinding.rs`'s tests — no real photo
    needed to check a Y-flip or ring-closure): Y-flip matches hand-computed coordinates, a 3-point polygon
    ring closes to 4 coordinates with first==last, an unread control's `code` serializes as JSON `null` (not
    omitted or a placeholder string), a route's `terrain_breakdown` map round-trips correctly, and the
    top-level `properties` block carries the right photo metadata.
  - **Then verified against all 4 real in-scope photos** (`tests/analyze.rs`, extended): the produced string
    parses as valid JSON, is a `FeatureCollection`, `properties.source_photo`/`image_width_px` match the
    call's own inputs/outputs, and the feature count is at least the sum of polygons+paths+controls+legs
    (a floor, not an exact count, since start/finish/route features are conditional). Additionally spot
    rendered one photo's full GeoJSON and eyeballed representative features directly (a `course_leg`
    `LineString`, the `demo_route` feature's `cost`/`recomputed_cost`/`terrain_breakdown`, and a `water`
    `Polygon`'s ring) via a throwaway example binary, then deleted it — confirmed ring closure and
    Y-flipped coordinates look right on real data, not just the synthetic tests.
  - `main.dart`'s demo screen now passes the picked file's name through as `sourceFilename` and has a "Save
    GeoJSON..." button (`FilePicker.platform.saveFile` + `dart:io`'s `File.writeAsString` — this app is
    desktop-only so far, no web-safety concern from using `dart:io` here).

  This completes the Phase 0 pipeline's full port (`preprocessing` -> `segmentation` + `course_detection` ->
  `cost_grid` -> `pathfinding` -> `vectorize`) into Rust/`analyze_map`, matching `run_pipeline.py` end to
  end except for OCR (see `course_detection.rs`'s module doc) and the manual-correction UI (unbuilt in
  either codebase, explicitly Phase 1 scope per `PHASE0_HANDOFF.md`).

  `app/lib/main.dart`'s demo screen (`AnalyzeScreen`) overlays segmentation/course/route results directly
  on the rectified photo via a `CustomPainter`, and went through two rounds of fixes after being actually
  used, not just built:
  - **Overlay/image misalignment, checked directly**: `Image.memory` with explicit `width`/`height` but no
    `fit` defaults to `BoxFit.scaleDown` (confirmed by reading `flutter/lib/src/painting/decoration_image.dart`
    directly rather than guessing) — it gracefully downscales the displayed image to fit the window, but
    `CustomPaint` does **not** rescale its own drawing commands to match a shrunk canvas. Painting overlay
    points in raw full-resolution coordinates against a canvas that got shrunk to fit the window is what
    caused the overlay to visibly drift away from the image. Fixed with a `LayoutBuilder` computing one
    shared `scale = min(1, availableWidth / result.width)`, applied to both the displayed image size and,
    inside the painter, via `canvas.scale(scale)` — and since `canvas.scale` also shrinks stroke widths
    along with positions, every `Paint`'s `strokeWidth` is expressed as `desiredScreenPx / scale` so lines
    stay a constant on-screen thickness regardless of window size or source photo resolution.
  - **Detection noise on a real, uncalibrated photo is genuinely bad, checked directly on map0.jpg**: with
    the demo's deliberately-uncalibrated `analyzeMap` call (empty `legendBoxes`, default `houghParam2`),
    path detection produces ~1170-1419 segments scattered across the whole photo (not tracing real trails —
    it's picking up contour lines and vegetation-symbol edges, same root cause as `_path_lines`'s own
    "draft quality" framing in `segmentation.rs`), and `rock` over-triggers on real dense vegetation/contour
    texture (500-800+ polygons on one photo). Legend-box calibration measurably reduces this (`rock` 821→513
    polygons, `paths` 1419→1170 on map0.jpg with vs. without the Python prototype's known-good boxes for
    that file) but doesn't fix the bulk of it — most of the noise is inherent heuristic over-sensitivity on
    real terrain, not missing calibration, and was verified by direct before/after comparison via a
    throwaway debug binary (rendered both versions, compared polygon counts and visually), not assumed.
    This is consistent with the Python prototype's own "draft quality... human cleans up in the
    manual-correction UI" framing (see Project Context above) — the manual-correction UI doesn't exist yet,
    so this rough-looking overlay is an honest current-state snapshot, not a regression introduced by the
    Rust port or the Flutter demo screen.
  - **Interim mitigation added to the demo screen** (not touching the CV pipeline, deliberately, absent new
    evidence for a specific threshold fix — see the `out_of_bounds` finding above for why): a `_LayerFilterBar`
    with a `FilterChip` per layer (terrain classes + paths/legs/controls/route, each labeled with its live
    detected count, e.g. `out_of_bounds (0)` — so an empty class is now visibly explicit rather than silently
    invisible) to show/hide layers, plus a min-polygon-area slider to hide small noise blobs. Polygons are
    now filled with a translucent color (not just a thin outline, which was easy to miss against a busy
    printed map) and `paths` moved from black (blended into these maps' own printed black path ink) to cyan.
  - **A `shouldRepaint` gotcha hit while adding the filter chips**: `_hiddenLayers` was originally a `final
    Set<String>` mutated in place (`.add`/`.remove`) on toggle. `CustomPainter.shouldRepaint` compares the
    *old* painter instance's field against the *new* one; since both referenced the same mutated-in-place
    `Set` object, the comparison never detected a change and toggling a chip silently failed to repaint.
    Fixed by replacing the field with a fresh `Set` (`_hiddenLayers.toSet()..add/remove(key)`) on every
    toggle instead of mutating in place — worth remembering for any other mutable-collection state driving
    a `CustomPainter` in this codebase.

  Uses the `opencv` crate (Rust bindings, `twistedfall/opencv-rust`) via FFI to a real OpenCV install —
  this is genuinely new infrastructure, not something `flutter_rust_bridge_codegen create` set up. Needed
  on top of the base Flutter/Rust toolchain above:
  - **LLVM/libclang** (`winget install LLVM.LLVM`) — `opencv` crate's `build.rs` runs `bindgen` against
    the OpenCV C++ headers, which needs `libclang.dll`.
  - **OpenCV itself**, prebuilt (not built from source — a vcpkg from-source build was ruled out purely on
    time, not correctness). Downloaded directly from the `opencv/opencv` GitHub releases (not Chocolatey:
    this machine's `choco` isn't running elevated and installing to `C:\ProgramData\chocolatey` needs
    admin; the GitHub release `.exe` is a self-extracting archive that unpacks anywhere without admin).
    Currently at `C:\Users\AlexandrGeorgiev\opencv4_tools\opencv\build\` (Windows) and
    `C:\Users\AlexandrGeorgiev\opencv_tools_android_4\OpenCV-android-sdk\` (Android, downloaded and
    extracted but **not yet wired into the Android/Gradle build** — Windows was gotten working first and
    Android is follow-up work, see below).
  - **Checked directly, don't re-attempt OpenCV 5.0.0 on Windows**: the `opencv/opencv` GitHub release
    `5.0.0`'s prebuilt `opencv_world500.dll` and its own bundled headers are out of sync — several
    `cv::_OutputArray` methods (`fit`, `fitSameSize`) that the shipped `mat.hpp` declares (and that
    `opencv-rust`'s generated bindings call into) aren't actually exported from that specific DLL, so
    linking `rust_core` fails with `LNK2019` unresolved externals. Confirmed by `dumpbin /exports` on the
    DLL. This looks like a real packaging bug in that specific release asset, not a config mistake on this
    end. Fix was downgrading to the `4.14.0` release line (latest stable 4.x), which links cleanly and is
    what `opencv-rust` has years of testing against — also avoids OpenCV 5's `imgproc` -> `geometry` module
    split (`approxPolyDP`/`convexHull`/`contourArea`/`minAreaRect`/`getPerspectiveTransform`/`boxPoints`
    moved to a new `geometry` module in 5.x; `findContours`/`Canny`/`dilate`/`cvtColor`/`warpPerspective`
    did not move), which would otherwise need `#[cfg(...)]`-gated imports to stay portable across the two
    major versions.
  - **Required env vars** (persisted as user-level env vars via `[Environment]::SetEnvironmentVariable`,
    so new shells pick them up without re-setting): `OPENCV_LINK_LIBS=opencv_world4140`,
    `OPENCV_LINK_PATHS=C:\Users\AlexandrGeorgiev\opencv4_tools\opencv\build\x64\vc16\lib`,
    `OPENCV_INCLUDE_PATHS=C:\Users\AlexandrGeorgiev\opencv4_tools\opencv\build\include` (both consumed by
    `opencv-rust`'s `build.rs`/cargo, backslashes are fine here),
    `OPENCV_BIN_DIR=C:/Users/AlexandrGeorgiev/opencv4_tools/opencv/build/x64/vc16/bin` (used only by the
    CMake DLL-bundling step below — **must use forward slashes here specifically**: a backslash-heavy
    value breaks when CMake interpolates it into a generated `.cmake` install script
    (`Invalid character escape '\U'`), since CMake treats backslash as its own string-escape character;
    checked directly, this is what `OPENCV_BIN_DIR` hit before switching to forward slashes), and
    `LIBCLANG_PATH=C:\Program Files\LLVM\bin`.
  - **Runtime DLL, not just link-time `.lib`**: linking `rust_core` only needs `OPENCV_LINK_PATHS`'s
    `.lib`; *running* anything that loads `rust_core.dll` (the built app, or `cargo test`) additionally
    needs the actual `opencv_world4140.dll` discoverable — either on `PATH` or copied next to the
    executable — or it fails to start with `STATUS_DLL_NOT_FOUND` despite having built successfully.
    Checked directly: `flutter build windows` succeeded while still silently producing a
    non-starting `app.exe`, because cargokit's own DLL-bundling only knows about `rust_core.dll`, not
    its transitive `opencv_world4140.dll` dependency. Fixed by a small addition to `app/windows/CMakeLists.txt`
    (search `OPENCV_BIN_DIR`) that installs `$ENV{OPENCV_BIN_DIR}/$ENV{OPENCV_LINK_LIBS}.dll` next to
    `app.exe` at build time, gated on those two env vars being set — **this is a hand-edit to a
    `flutter create`-generated file**, same caveat as the Gradle patches above: re-check after any
    regeneration. For `cargo test`/`cargo build` run directly (not through `flutter build`), the OpenCV
    `bin` dir also needs to be on `PATH` for the same reason.
  - **Android is not wired up yet.** The OpenCV Android SDK (prebuilt per-ABI `.so`s) is downloaded and
    extracted (see path above) but nothing in `app/rust_builder`'s Gradle/cargokit build currently points
    at it — cargokit invokes `cargo build --target <abi>-linux-android` per ABI itself, and each of those
    needs its own `OPENCV_LINK_LIBS`/`OPENCV_LINK_PATHS`/`OPENCV_INCLUDE_PATHS` pointed at that ABI's
    slice of the Android SDK (plus the resulting `.so` bundled into the APK's `jniLibs`), which is
    meaningfully more setup than the Windows env vars above and hasn't been attempted yet — a distinct
    follow-up task, not assumed-done infrastructure.
  - **GUI verification caveat**: this repo is sometimes worked on from a non-interactive background
    session with no attached Windows desktop, where a launched GUI app never renders a visible window
    (`MainWindowHandle` stays `0`) even though it's running correctly. Don't treat a blank/handle-0 window
    as a crash in that situation — instead confirm correctness headlessly (`cargo test` against real
    photos, as `rectify_photo.rs` does, and/or checking via `Get-Process ... | Select Modules` that the
    expected DLLs actually loaded) and say plainly that visual GUI confirmation wasn't possible, rather
    than claiming to have "seen" the app run.

Key facts from `prompt.txt` that matter when working in `python_prototype/`:

- **`python_prototype/` is the Python CV/path-finding prototype only** (`prompt.txt` phase 0: "prove
  segmentation → cost-grid → path works"). Production mobile code is Flutter (Dart) + Rust (via
  `flutter_rust_bridge`); Python here is prototyping only, not something that gets ported/shipped as-is.
  Don't blur the two worlds without an explicit reason.
- The **fixed stack** for the eventual product (Flutter/Rust/OpenCV-FFI/TFLite/ML Kit/SQLite) is decided —
  don't propose alternatives unless explicitly asked, and don't design this Python code as if it were the
  production implementation.
- The internal map format is a **GeoJSON-like structure** (polygons for terrain classes + points for
  controls). `.omap`/`.ocd` (OpenOrienteeringMapper/OCAD) are a separate, unbuilt *input* bypass path for
  already-digitized maps — not something this pipeline generates.
- **Phase 0 is now complete**: photo → vector map → cost-grid → a demo least-cost route, proving the
  "segmentation → cost-grid → path" chain end to end (`pipeline/cost_grid.py`, `pipeline/pathfinding.py`).
  **Don't skip ahead by phase** past this: Fast Marching Method, slope-aware cost from horizontals, live
  GPS, and ML segmentation are later phases (1-4) — don't pull that work into a phase-0 task "while you're
  at it." Note the demo route is a **connectivity proof, not real course routing** — there's no
  control-sequencing/course-graph yet (`course_detection.CourseResult.controls` is an unordered list), so
  `run_pipeline.py` demos "start → nearest control" when a start is found, or the first two detected
  controls otherwise — which in practice is *always* the fallback right now, since `detect_start_triangle`
  reliably returns `None` on all four in-scope photos (a real, checked-directly limitation, not a bug — see
  its docstring and the `course_detection.py` section below before trying to "fix" it again).
  Building a real ordered multi-leg course route needs the manual-correction UI that's explicitly Phase 1
  scope, since automatic control-sequencing from a photo alone isn't reliable enough to trust un-corrected.
- **No labeled ground truth exists** for the sample photos in `testData/` — don't fabricate accuracy
  numbers or synthetic "ground truth" to make something look validated. Quality is judged by the QA
  overlay PNGs (visual) and a couple of manually-counted control baselines (`config.MANUAL_KP_COUNTS`),
  which are explicitly one-observer counts, not audited data.
- Before writing non-trivial code, state the plan and key assumptions first.

## Scope: which test photos are "in scope"

`python_prototype/testData/` has 9 real photographed orienteering maps, but they're **three different map genres** and only
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

All commands below are run from the **repo root**, not from inside `python_prototype/`.

Environment: Python 3.14, venv at `python_prototype/.venv/` (already created). No `requirements.txt` yet —
the dependency set actually installed is:

```
opencv-python-headless numpy shapely scikit-image pytesseract matplotlib pytest Pillow
```

Windows (PowerShell/Git Bash), always use the venv's own interpreter rather than relying on activation:

```
python_prototype/.venv/Scripts/python.exe -m pip install opencv-python-headless numpy shapely scikit-image pytesseract matplotlib pytest Pillow
```

Run the pipeline on one photo (writes `python_prototype/output/<name>.geojson` + `python_prototype/output/<name>_qa.png`):

```
python_prototype/.venv/Scripts/python.exe python_prototype/scripts/run_pipeline.py python_prototype/testData/map0.jpg
python_prototype/.venv/Scripts/python.exe python_prototype/scripts/run_pipeline.py python_prototype/testData/map0.jpg --no-ocr --out-dir python_prototype/output
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
python_prototype/.venv/Scripts/python.exe -m pytest python_prototype/tests/ -q
python_prototype/.venv/Scripts/python.exe -m pytest python_prototype/tests/test_geometry.py::test_dedupe_finish_merges_close_concentric_pair -q
```

There's no lint/format tooling configured in this repo.

## Architecture: the pipeline

All paths in this section are relative to `python_prototype/`. `scripts/run_pipeline.py:run()` wires the
stages together; each stage is its own module under `pipeline/`:

```
preprocessing.preprocess_image(path)
  → segmentation.default_valid_mask()  (legend/title exclusion from config.LEGEND_EXCLUDE_BOXES)
  → course_detection.detect_course()   (needs source_filename for config.HOUGH_PARAM2 lookup)
  → segmentation.segment_terrain()     (masked to exclude course ink so it isn't read as terrain fill)
  → cost_grid.build_cost_grid()        (terrain polygons + paths -> per-pixel traversal cost, config.TERRAIN_COST)
  → pathfinding.find_route()           (skimage.graph.route_through_array between a demo pair of points)
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

`detect_start_triangle` reliably returns `None` on all four in-scope photos, and that's the correct,
checked-directly answer, not a bug — see its docstring for the full account. Five separate approaches to
actually finding the real start triangle were tried and each failed for a concrete, verified reason: (1) an
unguarded shape-only search accepts noise blobs 30-220px² as "equilateral enough" when a real triangle at
these files' print scale should be ~2000-4000px²; (2) recalibrating the ink-color mask tighter using HSV
sampled from this file's own confirmed control-circle ink doesn't help — real course ink and false
positives (a road, a contour-line tangle, a sponsor logo) are statistically indistinguishable in HSV, even
after (3) document-scanner-style local illumination flattening, which rules out "it's just bad white
balance"; (4) comparing local stroke thickness (control-ring ink vs. rest of the mask) shows no signal
either — both come out to the same 4px median; (5) rotation-invariant template matching against a
synthetic triangle at the expected size found a strong, 120°-symmetric peak that looked promising until
cropped — it was a course leg crossing a kink in an area-boundary line, not a triangle. A full manual scan
of the rectified photo (including every `LEGEND_EXCLUDE_BOXES` region, in case the triangle was hidden
under a hand-excluded logo) didn't turn up an obvious separate triangle mark either. Given all of that,
`config.START_TRIANGLE_AREA_RATIO_RANGE`/`START_TRIANGLE_MIN_EQUILATERAL_SCORE` make the detector fail
closed (return `None`) instead of confidently reporting noise — the same "don't fabricate, return `None`"
principle `ocr_control_code` already uses. Reliably finding the real triangle on these photos would need a
fundamentally different approach (e.g. a trained symbol classifier) — Phase-3 ML-segmentation territory,
not a Phase-0 CV heuristic. Don't re-attempt any of the five approaches above without new evidence for why
they'd work differently next time.

**`vectorize.py`** — assembles segmentation + course results into one GeoJSON `FeatureCollection`, in the
rectified image's own local pixel coordinates (no real-world geo-referencing — out of scope by design, see
`prompt.txt`). Y is flipped (`height - y`) so the output is genuinely GeoJSON-convention (Y up), not
image-convention (Y down) mislabeled as GeoJSON. Carries a non-standard top-level `properties` block
(source photo, scale, detected line spacing) for `visualize.py` and, eventually, a Flutter renderer to
overlay the vector data back onto the photo without distortion.

**`cost_grid.py`** — rasterizes `segmentation.SegmentationResult` (terrain polygons + path lines) into a
single float array, one traversal cost per pixel, in the same pixel coordinate system as the working image
(Y-down — this runs before `vectorize.py`'s GeoJSON Y-flip). Costs come from `config.TERRAIN_COST`, which is
a **qualitative ISOM-convention ordering** (path cheapest, then clearing, forest, rock, thicket, with
water/out-of-bounds strongly avoided) — not a measured running-speed model, since no field data exists to
calibrate real speeds against (same "don't fabricate validated numbers" reasoning as `MANUAL_KP_COUNTS`).
Area classes are drawn in a fixed order (`_AREA_DRAW_ORDER`) so that if polygons from different classes
ever overlap, the stricter/more-expensive class wins, not the cheaper one. Pixels outside any detected
polygon default to a neutral clearing-like cost (`config.DEFAULT_TERRAIN_COST`) rather than acting as a
barrier, since those gaps are a segmentation artifact, not real terrain; pixels outside the valid (paper)
mask get a high but finite cost (`config.OUTSIDE_VALID_MASK_COST`) rather than `inf`, so a start/end point
landing just outside the valid mask from rectification slop doesn't make path-finding fail outright.

**`pathfinding.py`** — thin wrapper around `skimage.graph.route_through_array` (a Dijkstra-family grid
shortest-path solver; `scikit-image` was already an installed dependency, and this is the exact function
`prompt.txt`'s fixed stack names for the Python prototype's routing logic — `scikit-fmm`/Fast Marching is a
later phase and isn't installed here). `find_route(cost, start_xy, end_xy)` clamps out-of-bounds points into
the grid rather than raising, and returns the pixel-coordinate route plus its total cost. There is no
control-sequencing/course-graph in this repo (see Project context above), so `run_pipeline.py`'s
`_pick_demo_route_endpoints` only ever demos a single two-point leg (start → nearest control, or the first
two detected controls) — proof the grid is connected and pathable, not a real course route. The resulting
route becomes a `role: "demo_route"` `LineString` feature in the GeoJSON output and a yellow polyline on the
QA overlay.

**`config.py`** is where all the per-file empirical calibration lives (`PAGE_ROTATION_K`,
`LEGEND_EXCLUDE_BOXES`, `HOUGH_PARAM2`, `MANUAL_KP_COUNTS`) — when a photo's rectification or exclusion
zones change (e.g. after a `preprocessing.py` fix), these need re-checking; they were hand-positioned by
looking at that specific file's rectified/QA output, not derived from a formula. `PAGE_ROTATION_K` in
particular changes the *geometry* every other per-file constant is expressed in (rotating swaps width/
height and moves every fixed-position box) — if it ever changes for a file, `LEGEND_EXCLUDE_BOXES` and
`HOUGH_PARAM2` for that file need re-deriving from scratch against the newly-corrected image, not adjusted
by formula from the old ones.

## Testing

Paths below are relative to `python_prototype/`. Three tiers, deliberately not one (see `tests/*.py`
docstrings for the reasoning):

- `test_geometry.py` — synthetic inputs, exact-match assertions on deterministic geometry helpers
  (simplify, `make_valid` handling, mask→polygon, Hough-family shape classification).
- `test_cost_grid.py` / `test_pathfinding.py` — synthetic inputs, exact-match assertions on
  `cost_grid.build_cost_grid` (terrain-cost rasterization, draw-order/overlap behavior, valid-mask
  exclusion) and `pathfinding.find_route` (prefers a cheap corridor over an expensive wall, direct route on
  a uniform grid, out-of-bounds endpoint clamping) — no real photo needed to exercise a grid shortest-path
  search, same reasoning as `test_geometry.py`.
- `test_pipeline_golden.py` — real photos (all four `config.IN_SCOPE_FILES`), but *invariant* checks
  (aspect ratio sanity, control count within a tolerance band of `MANUAL_KP_COUNTS`, segmented area within a
  plausible fraction of the valid mask, **and now: a demo route is actually found and stays in-bounds**)
  rather than exact-match, since there's no pixel-level ground truth or ground-truth route.
- `test_smoke.py` — parametrized over `config.IN_SCOPE_FILES`, just asserts the full pipeline (now
  including cost-grid + routing) runs without raising and produces non-empty output.

When you change anything in `preprocessing.py` or `segmentation.py`'s masking, re-run the full pipeline on
all of `IN_SCOPE_FILES` and eyeball the `_qa.png` outputs before trusting the numbers — several real bugs
here (a silently-dropped `GeometryCollection`, a wrongly-cropped paper quad) passed a casual glance at
counts and only showed up on the rendered overlay.
