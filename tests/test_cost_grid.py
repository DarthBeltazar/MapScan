"""Deterministic unit tests on synthetic data for pipeline/cost_grid.py --
same reasoning as test_geometry.py: exact-match assertions on synthetic
inputs, since these grid-building rules are exact rasterization, not CV
heuristics that need a real photo to exercise.
"""

from __future__ import annotations

import numpy as np
from shapely.geometry import LineString, Polygon

from pipeline.config import DEFAULT_TERRAIN_COST, OUTSIDE_VALID_MASK_COST, TERRAIN_COST
from pipeline.cost_grid import build_cost_grid
from pipeline.segmentation import SegmentationResult


def test_default_cost_fills_ungrouped_background():
    seg = SegmentationResult()
    result = build_cost_grid(seg, (20, 20))
    assert result.cost.shape == (20, 20)
    assert np.all(result.cost == DEFAULT_TERRAIN_COST)


def test_area_class_paints_its_terrain_cost():
    seg = SegmentationResult()
    seg.polygons["thicket"] = [Polygon([(2, 2), (2, 8), (8, 8), (8, 2)])]
    result = build_cost_grid(seg, (10, 10))

    assert result.cost[5, 5] == TERRAIN_COST["thicket"]
    # Outside the polygon stays background/default.
    assert result.cost[0, 0] == DEFAULT_TERRAIN_COST


def test_overlapping_area_classes_stricter_class_wins():
    seg = SegmentationResult()
    seg.polygons["clearing"] = [Polygon([(0, 0), (0, 10), (10, 10), (10, 0)])]
    seg.polygons["water"] = [Polygon([(3, 3), (3, 7), (7, 7), (7, 3)])]
    result = build_cost_grid(seg, (10, 10))

    assert result.cost[5, 5] == TERRAIN_COST["water"]
    assert result.cost[1, 1] == TERRAIN_COST["clearing"]


def test_path_line_is_drawn_cheap_over_area_class():
    seg = SegmentationResult()
    seg.polygons["forest"] = [Polygon([(0, 0), (0, 20), (20, 20), (20, 0)])]
    seg.paths = [LineString([(0, 10), (19, 10)])]
    result = build_cost_grid(seg, (20, 20))

    assert result.cost[10, 10] == TERRAIN_COST["path"]
    assert result.cost[1, 1] == TERRAIN_COST["forest"]


def test_valid_mask_excludes_outside_area():
    seg = SegmentationResult()
    valid = np.zeros((10, 10), dtype=bool)
    valid[2:8, 2:8] = True
    result = build_cost_grid(seg, (10, 10), valid_mask=valid)

    assert result.cost[0, 0] == OUTSIDE_VALID_MASK_COST
    assert result.cost[5, 5] == DEFAULT_TERRAIN_COST
