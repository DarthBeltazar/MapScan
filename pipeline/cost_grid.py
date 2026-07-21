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

import math
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

# Parallel class labels for CostGridResult.class_grid, in the same paint
# order as build_cost_grid itself (each later entry overwrites earlier ones
# at the same pixel, same as the cost array). "default_gap" is background
# never claimed by any polygon, path, or the valid mask -- a segmentation
# gap, not a real terrain reading (see config.DEFAULT_TERRAIN_COST).
CLASS_DEFAULT_GAP = "default_gap"
CLASS_OUTSIDE_VALID_MASK = "outside_valid_mask"
CLASS_NAMES: tuple[str, ...] = (CLASS_DEFAULT_GAP,) + _AREA_DRAW_ORDER + ("path", CLASS_OUTSIDE_VALID_MASK)
_CLASS_CODE = {name: i for i, name in enumerate(CLASS_NAMES)}


@dataclass
class CostGridResult:
    cost: np.ndarray  # float32, shape (h, w), traversal cost per pixel
    # uint8, same shape -- which paint layer actually set each pixel's cost
    # (see CLASS_NAMES). Lets route_terrain_breakdown (and any other QA)
    # answer "what terrain did this route actually cross" without having to
    # reverse-engineer it from cost values alone, which collide across
    # classes on purpose (e.g. default_gap and clearing share a cost).
    class_grid: np.ndarray


def build_cost_grid(
    seg: SegmentationResult, shape: tuple[int, int], valid_mask: np.ndarray | None = None,
) -> CostGridResult:
    h, w = shape[:2]
    cost = np.full((h, w), DEFAULT_TERRAIN_COST, dtype=np.float32)
    class_grid = np.full((h, w), _CLASS_CODE[CLASS_DEFAULT_GAP], dtype=np.uint8)

    for cls in _AREA_DRAW_ORDER:
        polys = seg.polygons.get(cls, [])
        if not polys:
            continue
        mask = np.zeros((h, w), dtype=np.uint8)
        for poly in polys:
            _fill_polygon(mask, poly)
        mask_bool = mask.astype(bool)
        cost[mask_bool] = TERRAIN_COST[cls]
        class_grid[mask_bool] = _CLASS_CODE[cls]

    if seg.paths:
        path_mask = np.zeros((h, w), dtype=np.uint8)
        for line in seg.paths:
            _draw_line(path_mask, line)
        path_mask_bool = path_mask.astype(bool)
        cost[path_mask_bool] = TERRAIN_COST["path"]
        class_grid[path_mask_bool] = _CLASS_CODE["path"]

    if valid_mask is not None:
        outside = ~valid_mask.astype(bool)
        cost[outside] = OUTSIDE_VALID_MASK_COST
        class_grid[outside] = _CLASS_CODE[CLASS_OUTSIDE_VALID_MASK]

    return CostGridResult(cost=cost, class_grid=class_grid)


def route_terrain_breakdown(
    class_grid: np.ndarray, points: list[tuple[float, float]],
) -> dict[str, float]:
    """What terrain a route (a list of (x, y) pixel points, e.g.
    RouteResult.points) actually crossed, as a fraction of its total
    geometric length per CLASS_NAMES entry. This is the human-legible
    correctness check pathfinding's raw cost number can't give you on its
    own: a route with a low cost is only reassuring if it's low *because*
    it's mostly on path/clearing, not because it cut a short chord through
    an under-detected gap. Each step's length is attributed to the terrain
    class at its midpoint pixel (consistent with how MCP_Geometric itself
    treats a step as spanning both endpoint pixels -- see
    pathfinding._geometric_path_cost)."""
    h, w = class_grid.shape
    totals: dict[str, float] = {}
    for (x1, y1), (x2, y2) in zip(points[:-1], points[1:]):
        dist = math.hypot(x2 - x1, y2 - y1)
        if dist == 0:
            continue
        mx = int(round(min(max((x1 + x2) / 2.0, 0), w - 1)))
        my = int(round(min(max((y1 + y2) / 2.0, 0), h - 1)))
        name = CLASS_NAMES[int(class_grid[my, mx])]
        totals[name] = totals.get(name, 0.0) + dist
    total_len = sum(totals.values())
    if total_len == 0:
        return {}
    return {name: length / total_len for name, length in totals.items()}


def _fill_polygon(mask: np.ndarray, poly: Polygon) -> None:
    exterior = np.array(poly.exterior.coords, dtype=np.int32)
    cv2.fillPoly(mask, [exterior], color=1)


def _draw_line(mask: np.ndarray, line: LineString) -> None:
    pts = np.array(line.coords, dtype=np.int32)
    cv2.polylines(mask, [pts], isClosed=False, color=1, thickness=PATH_COST_LINE_WIDTH_PX)
