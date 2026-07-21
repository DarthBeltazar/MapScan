"""Smoke test: the full pipeline must run without raising on every in-scope
file (config.IN_SCOPE_FILES), even where output quality is mediocre. OCR is
skipped -- Tesseract's binary isn't installed on this machine (see plan),
and OCR failure is already handled gracefully inside course_detection.py,
so this isn't what a smoke test needs to cover.
"""

from __future__ import annotations

import os
import sys

import pytest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from pipeline.config import IN_SCOPE_FILES
from scripts.run_pipeline import run

TESTDATA = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "testData")


@pytest.mark.parametrize("filename", IN_SCOPE_FILES)
def test_pipeline_runs_without_exception(filename, tmp_path):
    path = os.path.join(TESTDATA, filename)
    fc = run(path, out_dir=str(tmp_path), run_ocr=False)

    assert fc["type"] == "FeatureCollection"
    assert len(fc["features"]) > 0
    stem = os.path.splitext(filename)[0]
    assert (tmp_path / f"{stem}.geojson").exists()
    assert (tmp_path / f"{stem}_qa.png").exists()
