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
import sys

import numpy as np
import pytest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from pipeline.config import MANUAL_KP_COUNTS, TERRAIN_COST
from pipeline.cost_grid import build_cost_grid
from pipeline.course_detection import detect_course
from pipeline.pathfinding import find_route
from pipeline.preprocessing import preprocess_image
from pipeline.segmentation import default_valid_mask, segment_terrain
from scripts.run_pipeline import _pick_demo_route_endpoints

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


def test_demo_route_found_end_to_end_on_real_photo(golden_photo):
    """Phase-0's "segmentation -> cost-grid -> path" proof, run against a
    real photo rather than synthetic data (unit-level cost-grid/pathfinding
    correctness is covered in test_cost_grid.py/test_pathfinding.py). Only
    checks the route is found and sane -- there's no ground-truth route to
    compare against, same reasoning as the rest of this file."""
    name, pre, mask, course = golden_photo
    endpoints = _pick_demo_route_endpoints(course)
    if endpoints is None:
        pytest.skip(f"{name}: not enough detected controls/start for a demo route")

    seg_mask = mask & ~course.ink_mask
    seg = segment_terrain(pre.image, seg_mask)
    cost_grid = build_cost_grid(seg, pre.image.shape, valid_mask=mask)

    route = find_route(cost_grid.cost, endpoints[0], endpoints[1])

    assert len(route.points) >= 2
    assert np.isfinite(route.cost)
    assert route.cost > 0
    h, w = pre.image.shape[:2]
    assert all(0 <= x <= w - 1 and 0 <= y <= h - 1 for x, y in route.points)

    # A demo route between two on-land course points (start/controls, never
    # placed in water) shouldn't need to cross the lake or leave the paper --
    # both are strongly-avoided-but-finite costs (config.OUTSIDE_VALID_MASK_COST
    # in particular is finite on purpose, see its docstring, so a route
    # crossing it wouldn't otherwise raise or get clamped away). Catching this
    # here needs a real photo's actual terrain layout -- the synthetic grids
    # in test_pathfinding.py can't exercise it.
    barrier_costs = {TERRAIN_COST["water"], TERRAIN_COST["out_of_bounds"]}
    on_barrier = [
        (x, y) for x, y in route.points
        if cost_grid.cost[int(round(y)), int(round(x))] in barrier_costs
    ]
    assert not on_barrier, f"{name}: demo route crosses water/out-of-bounds at {on_barrier[:5]}"
