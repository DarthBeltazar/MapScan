"""Deterministic unit tests on synthetic data -- no photos involved, so
these are exact-match assertions, not the invariant-style checks used for
the real test photos in test_pipeline_golden.py (see plan's testing
section on why that distinction matters here)."""

from __future__ import annotations

import numpy as np
from shapely.geometry import Polygon
from shapely.validation import make_valid

from pipeline.course_detection import Control, _dedupe_finish
from pipeline.segmentation import MIN_POLYGON_AREA_PX, _mask_to_polygons, build_valid_mask
from pipeline.vectorize import _flip_y


def test_simplify_preserves_topology_and_rough_area():
    # A near-rectangular polygon with lots of redundant near-collinear
    # points along one edge -- simplification should collapse those points
    # without changing the shape's identity or blowing up its area.
    edge_points = [(x, 0.0) for x in range(0, 101, 2)]
    poly = Polygon(edge_points + [(100, 50), (0, 50)])
    simplified = poly.simplify(2.0, preserve_topology=True)

    assert simplified.is_valid
    assert len(simplified.exterior.coords) < len(poly.exterior.coords)
    assert abs(simplified.area - poly.area) / poly.area < 0.05


def test_make_valid_fixes_self_intersecting_bowtie():
    bowtie = Polygon([(0, 0), (10, 10), (10, 0), (0, 10), (0, 0)])
    assert not bowtie.is_valid

    fixed = make_valid(bowtie)
    assert fixed.is_valid
    assert fixed.area > 0


def test_flip_y_matches_height_minus_y():
    flip = _flip_y(height=100)
    assert flip(5, 0) == (5, 100)
    assert flip(5, 100) == (5, 0)
    assert flip(5, 40) == (5, 60)


def test_build_valid_mask_excludes_fractional_box():
    mask = build_valid_mask((100, 200), legend_boxes=[(0.0, 0.0, 0.5, 0.5)])
    assert mask.shape == (100, 200)
    # Excluded quadrant (top-left, per the fractional box).
    assert not mask[10, 10]
    # Everywhere else stays valid.
    assert mask[90, 190]
    assert mask[10, 190]
    assert mask[90, 10]


def test_build_valid_mask_extra_exclude_mask_combines():
    extra = np.zeros((10, 10), dtype=bool)
    extra[5, 5] = True
    mask = build_valid_mask((10, 10), legend_boxes=[], extra_exclude_mask=extra)
    assert not mask[5, 5]
    assert mask[0, 0]


def test_mask_to_polygons_filters_noise_keeps_real_blob():
    mask = np.zeros((100, 100), dtype=bool)
    mask[10:40, 10:40] = True  # a real 30x30 blob, well above the area floor
    mask[80, 80] = True        # a single stray pixel -- noise
    polys = _mask_to_polygons(mask)

    assert len(polys) == 1
    assert polys[0].area >= MIN_POLYGON_AREA_PX
    # The blob's centroid should land inside the real square, not the noise pixel.
    assert 10 <= polys[0].centroid.x <= 40
    assert 10 <= polys[0].centroid.y <= 40


def test_dedupe_finish_merges_close_concentric_pair():
    close_pair = [Control(x=100, y=100, radius=10), Control(x=101, y=99, radius=14)]
    far_control = Control(x=500, y=500, radius=12)
    remaining, finish = _dedupe_finish(close_pair + [far_control])

    assert finish is not None
    assert remaining == [far_control]


def test_dedupe_finish_no_pair_returns_all_and_none():
    controls = [Control(x=0, y=0, radius=10), Control(x=500, y=500, radius=10)]
    remaining, finish = _dedupe_finish(controls)

    assert finish is None
    assert remaining == controls
