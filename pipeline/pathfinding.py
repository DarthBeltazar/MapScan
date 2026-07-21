"""Least-cost path between two pixel points on a cost grid.

Phase-0 scope: this is the "-> path" half of prompt.txt phase 0's
"segmentation -> cost-grid -> path" proof. Uses
skimage.graph.route_through_array (a Dijkstra-family grid shortest-path
solver, already an installed dependency) -- prompt.txt's fixed stack names
this exact function for the Python prototype's path-finding logic, with
scikit-fmm/Fast Marching reserved for a later phase (not installed here, and
out of scope for phase 0).

There is no control-sequencing/course-graph in this repo yet (course_detection
only returns an unordered list of controls plus optional start/finish) --
building that, and letting a human correct it, is Phase 1 UI work. So the
route this module finds is a two-point demo: proof that the grid is
connected and pathable, not a real full-course route.
"""

from __future__ import annotations

import math
from dataclasses import dataclass

import numpy as np
from skimage.graph import route_through_array


@dataclass
class RouteResult:
    points: list[tuple[float, float]]  # (x, y) pixel coords, image convention (Y-down)
    cost: float
    # Independent cross-check on `cost` -- see _geometric_path_cost's
    # docstring. Not a second opinion from a different algorithm, but a
    # from-scratch resum of the same route against the same grid using
    # MCP_Geometric's own documented weighting; if this drifts from `cost`
    # by more than float rounding, something upstream (the grid passed in,
    # or a route/cost mismatch) is actually wrong, not just "approximate".
    recomputed_cost: float


def find_route(
    cost: np.ndarray, start_xy: tuple[float, float], end_xy: tuple[float, float],
) -> RouteResult:
    h, w = cost.shape
    start_rc = _xy_to_rc(start_xy, h, w)
    end_rc = _xy_to_rc(end_xy, h, w)
    indices, total_cost = route_through_array(
        cost, start_rc, end_rc, fully_connected=True, geometric=True,
    )
    points = [(float(c), float(r)) for r, c in indices]
    return RouteResult(
        points=points, cost=float(total_cost),
        recomputed_cost=_geometric_path_cost(cost, points),
    )


def _geometric_path_cost(cost: np.ndarray, points: list[tuple[float, float]]) -> float:
    """Resum a route's cost directly from the grid, independently of
    whatever route_through_array itself reported. Follows
    skimage.graph.MCP_Geometric's own documented per-step weighting (its
    docstring, verbatim): a step of Euclidean length d between two pixels
    costs `d * (cost[p1] + cost[p2]) / 2` -- half the step's length billed
    at each endpoint's cost. Summed over the whole route this should
    reproduce route_through_array's own `cost` return value to within float
    rounding; see RouteResult.recomputed_cost."""
    total = 0.0
    for (x1, y1), (x2, y2) in zip(points[:-1], points[1:]):
        r1, c1 = int(round(y1)), int(round(x1))
        r2, c2 = int(round(y2)), int(round(x2))
        dist = math.hypot(x2 - x1, y2 - y1)
        total += dist * (float(cost[r1, c1]) + float(cost[r2, c2])) / 2.0
    return total


def _xy_to_rc(xy: tuple[float, float], h: int, w: int) -> tuple[int, int]:
    x, y = xy
    row = int(round(min(max(y, 0), h - 1)))
    col = int(round(min(max(x, 0), w - 1)))
    return row, col
