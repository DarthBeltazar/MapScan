"""Golden-path invariant tests against real photos (config.MANUAL_KP_COUNTS
files: map0.jpg, map2.jpg, map4.jpg, map6.jpg).

Not exact-match: there is no labeled ground truth for these photos (see
plan's testing section), so these assert sanity invariants and a comparison
against a *manually counted* control baseline (config.MANUAL_KP_COUNTS --
one observer, not audited data; map4.jpg/map6.jpg's counts are weaker still,
see that constant's docstring). Treat this as a regression guard, not an
accuracy metric.
"""

from __future__ import annotations

import os

import pytest

from pipeline.config import MANUAL_KP_COUNTS
from pipeline.course_detection import detect_course
from pipeline.preprocessing import preprocess_image
from pipeline.segmentation import default_valid_mask, segment_terrain

TESTDATA = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "testData")
GOLDEN_FILES = list(MANUAL_KP_COUNTS)


@pytest.fixture(scope="module", params=GOLDEN_FILES)
def golden_photo(request):
    path = os.path.join(TESTDATA, request.param)
    pre = preprocess_image(path)
    mask = default_valid_mask(pre.image, request.param)
    course = detect_course(pre.image, mask, run_ocr=False, source_filename=request.param)
    return request.param, pre, mask, course


def test_paper_quad_found_with_sane_aspect_ratio(golden_photo):
    _name, pre, _mask, _course = golden_photo
    assert pre.quad_found
    h, w = pre.image.shape[:2]
    aspect = max(h, w) / min(h, w)
    # These are printed A3/A4-ish landscape or portrait sheets -- a wildly
    # off aspect ratio would mean rectification picked the wrong contour.
    assert 1.0 <= aspect <= 2.5


def test_control_count_within_tolerance_of_manual_baseline(golden_photo):
    name, _pre, _mask, course = golden_photo
    expected = MANUAL_KP_COUNTS[name]
    found = len(course.controls)
    tolerance = max(4, round(expected * 0.3))
    assert abs(found - expected) <= tolerance, (
        f"{name}: found {found} controls, manually counted {expected} "
        f"(tolerance {tolerance})"
    )


def test_segmentation_covers_a_reasonable_share_of_the_valid_area(golden_photo):
    _name, pre, mask, course = golden_photo
    seg_mask = mask & ~course.ink_mask
    seg = segment_terrain(pre.image, seg_mask)

    total_area = sum(p.area for polys in seg.polygons.values() for p in polys)
    valid_area = int(seg_mask.sum())
    # Empirically ~0.9-1.15 on the golden files (simplify() can slightly
    # grow boundaries) -- wide-ish margin since this is draft-quality
    # classification (see plan), but tight enough to catch a regression like
    # the GeometryCollection-handling bug this caught during development
    # (that silently dropped whole polygons, down to ~0.04-0.25 coverage).
    assert 0.5 * valid_area <= total_area <= 1.3 * valid_area
