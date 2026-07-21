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

from dataclasses import dataclass

import numpy as np
from skimage.graph import route_through_array


@dataclass
class RouteResult:
    points: list[tuple[float, float]]  # (x, y) pixel coords, image convention (Y-down)
    cost: float


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
    return RouteResult(points=points, cost=float(total_cost))


def _xy_to_rc(xy: tuple[float, float], h: int, w: int) -> tuple[int, int]:
    x, y = xy
    row = int(round(min(max(y, 0), h - 1)))
    col = int(round(min(max(x, 0), w - 1)))
    return row, col
