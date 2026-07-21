"""Terrain polygons + path lines -> a per-pixel traversal-cost grid.

Phase-0 scope (prompt.txt phase 0: prove "segmentation -> cost-grid -> path"
works): this rasterizes segmentation.SegmentationResult into a single float
array in the same pixel coordinate system as the working-resolution image
(Y-down, matching segmentation/course_detection output before vectorize.py's
GeoJSON Y-flip), so pathfinding.py can run a grid shortest-path search
directly against it. See config.TERRAIN_COST's docstring for why the cost
values are a qualitative ordering, not a measured speed model.
"""

from __future__ import annotations

from dataclasses import dataclass

import cv2
import numpy as np
from shapely.geometry import LineString, Polygon

from pipeline.config import DEFAULT_TERRAIN_COST, OUTSIDE_VALID_MASK_COST, PATH_COST_LINE_WIDTH_PX, TERRAIN_COST
from pipeline.segmentation import SegmentationResult

# Draw order for area classes: later entries paint over earlier ones, so if
# polygons from different classes ever overlap (segmentation doesn't
# guarantee disjointness), the stricter/more-expensive class wins rather than
# the cheaper one -- the safer failure mode for a "don't run through the lake"
# tool.
_AREA_DRAW_ORDER = ("clearing", "forest", "thicket", "rock", "marsh", "water", "out_of_bounds")


@dataclass
class CostGridResult:
    cost: np.ndarray  # float32, shape (h, w), traversal cost per pixel


def build_cost_grid(
    seg: SegmentationResult, shape: tuple[int, int], valid_mask: np.ndarray | None = None,
) -> CostGridResult:
    h, w = shape[:2]
    cost = np.full((h, w), DEFAULT_TERRAIN_COST, dtype=np.float32)

    for cls in _AREA_DRAW_ORDER:
        polys = seg.polygons.get(cls, [])
        if not polys:
            continue
        mask = np.zeros((h, w), dtype=np.uint8)
        for poly in polys:
            _fill_polygon(mask, poly)
        cost[mask.astype(bool)] = TERRAIN_COST[cls]

    if seg.paths:
        path_mask = np.zeros((h, w), dtype=np.uint8)
        for line in seg.paths:
            _draw_line(path_mask, line)
        cost[path_mask.astype(bool)] = TERRAIN_COST["path"]

    if valid_mask is not None:
        cost[~valid_mask.astype(bool)] = OUTSIDE_VALID_MASK_COST

    return CostGridResult(cost=cost)


def _fill_polygon(mask: np.ndarray, poly: Polygon) -> None:
    exterior = np.array(poly.exterior.coords, dtype=np.int32)
    cv2.fillPoly(mask, [exterior], color=1)


def _draw_line(mask: np.ndarray, line: LineString) -> None:
    pts = np.array(line.coords, dtype=np.int32)
    cv2.polylines(mask, [pts], isClosed=False, color=1, thickness=PATH_COST_LINE_WIDTH_PX)
