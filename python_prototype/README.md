# python_prototype — Phase 0 prototype (complete, frozen)

Proves the pipeline shape — photo → rectify → segment terrain/course → cost-grid →
least-cost path — works end to end on real photographed orienteering maps. This is a
CV/path-finding **prototype only**: the production app is [`../app/`](../app/)
(Flutter + Rust), not a port of this code. See [`../PHASE0_HANDOFF.md`](../PHASE0_HANDOFF.md)
for what carries forward conceptually into Phase 1, and [`../CLAUDE.md`](../CLAUDE.md)
for the full architecture writeup, per-module docstring pointers, and every
calibration constant's provenance.

## Layout

- `pipeline/` — `preprocessing` → `segmentation` + `course_detection` →
  `cost_grid` → `pathfinding` → `vectorize`, plus `config.py` (per-photo
  calibration) and `visualize.py` (QA overlay rendering).
- `scripts/run_pipeline.py` — runs the full pipeline on one photo.
- `testData/` — 9 real photographed maps; only 4 (`map0/2/4/6.jpg`, classic forest
  ISOM) are in scope for the current terrain taxonomy — see `../CLAUDE.md`'s scope
  note before extending to the others.
- `tests/` — synthetic-input exact-match tests for the algorithmic parts, plus
  invariant checks against the real in-scope photos (no pixel-level ground truth
  exists for these photos, so exact-match isn't possible there).

## Setup

Python 3.14, venv at `.venv/` (from the repo root):

```
python_prototype/.venv/Scripts/python.exe -m pip install opencv-python-headless numpy shapely scikit-image pytesseract matplotlib pytest Pillow
```

Tesseract-OCR binary (for control-code OCR, best-effort) via
`winget install UB-Mannheim.TesseractOCR`.

## Running

```
python_prototype/.venv/Scripts/python.exe python_prototype/scripts/run_pipeline.py python_prototype/testData/map0.jpg
python_prototype/.venv/Scripts/python.exe -m pytest python_prototype/tests/ -q
```

Writes `output/<name>.geojson` and `output/<name>_qa.png` (visual QA overlay — eyeball
this after touching `preprocessing.py`/`segmentation.py`, several real bugs here only
showed up on the rendered overlay, not in the counts).
