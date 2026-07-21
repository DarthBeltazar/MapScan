"""Deterministic unit tests on synthetic data -- no photos involved, so
these are exact-match assertions, not the invariant-style checks used for
the real test photos in test_pipeline_golden.py (see plan's testing
section on why that distinction matters here)."""

from __future__ import annotations

import cv2
import numpy as np
from shapely.geometry import Polygon
from shapely.validation import make_valid

from pipeline.course_detection import Control, _dedupe_finish, detect_start_triangle
from pipeline.segmentation import MIN_POLYGON_AREA_PX, _mask_to_polygons, build_valid_mask
from pipeline.vectorize import _flip_y


def _draw_triangle_outline(mask: np.ndarray, vertices, thickness: int = 5) -> None:
    mask_u8 = mask.astype(np.uint8)
    pts = np.array(vertices, dtype=np.int32)
    cv2.polylines(mask_u8, [pts], isClosed=True, color=1, thickness=thickness)
    mask[:] = mask_u8.astype(bool)


def _equilateral_triangle_vertices(cx: float, cy: float, side: float) -> list[tuple[float, float]]:
    r = side / np.sqrt(3)
    return [
        (cx + r * np.cos(np.deg2rad(-90 + k * 120)), cy + r * np.sin(np.deg2rad(-90 + k * 120)))
        for k in range(3)
    ]


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


# detect_start_triangle's size/shape gate (config.START_TRIANGLE_*): see its
# docstring for why an unguarded shape-only search was unreliable on real
# photos (noise blobs far smaller than a real triangle scored as
# "equilateral enough"). These use control circles of radius 36px, matching
# the median this gate was calibrated against on map0.jpg.
_CONTROLS_RADIUS_36 = [Control(x=300, y=300, radius=36), Control(x=300, y=100, radius=36)]


def test_detect_start_triangle_accepts_large_equilateral_triangle_at_expected_scale():
    mask = np.zeros((400, 400), dtype=bool)
    vertices = _equilateral_triangle_vertices(cx=100, cy=100, side=72)  # side == control diameter
    _draw_triangle_outline(mask, vertices)

    result = detect_start_triangle(mask, _CONTROLS_RADIUS_36)

    assert result is not None
    x, y = result
    assert abs(x - 100) < 10
    assert abs(y - 100) < 10


def test_detect_start_triangle_rejects_small_noise_triangle():
    mask = np.zeros((400, 400), dtype=bool)
    # Same scale/shape as an actual false positive measured on map0.jpg
    # (sides ~44.7/25.7/23.6px, area ~138px^2) -- far below a real triangle's
    # expected area at this control-circle scale (~2000-4000px^2).
    _draw_triangle_outline(mask, [(100, 80), (130, 95), (115, 120)], thickness=2)

    assert detect_start_triangle(mask, _CONTROLS_RADIUS_36) is None


def test_detect_start_triangle_rejects_skewed_triangle_at_right_scale():
    mask = np.zeros((400, 400), dtype=bool)
    # Right area range (~2200px^2) but a thin, elongated triangle, not
    # equilateral -- should fail the equilateral-score gate.
    _draw_triangle_outline(mask, [(50, 100), (150, 105), (100, 115)])

    assert detect_start_triangle(mask, _CONTROLS_RADIUS_36) is None


def test_detect_start_triangle_returns_none_with_no_controls_to_calibrate_against():
    mask = np.zeros((400, 400), dtype=bool)
    _draw_triangle_outline(mask, _equilateral_triangle_vertices(cx=100, cy=100, side=72))

    assert detect_start_triangle(mask, []) is None
