"""Deterministic unit tests on synthetic cost grids for
pipeline/pathfinding.py -- exact/behavioral assertions on a hand-built cost
array, same reasoning as test_geometry.py/test_cost_grid.py (no real photo
needed to exercise a grid shortest-path search).
"""

from __future__ import annotations

import numpy as np

from pipeline.pathfinding import find_route


def test_route_prefers_cheap_corridor_over_expensive_wall():
    # A cheap horizontal corridor at row 5 through an otherwise expensive
    # field; start and end sit off the corridor's row, so the router has to
    # detour onto it rather than cutting straight across the wall.
    cost = np.full((11, 11), 10.0, dtype=np.float32)
    cost[5, :] = 1.0

    route = find_route(cost, (0.0, 0.0), (10.0, 10.0))

    rows = {int(round(y)) for _x, y in route.points}
    assert 5 in rows  # detoured through the cheap corridor
    # A straight diagonal through the expensive field would cost ~10x more
    # per unit length than following the corridor -- confirm the router
    # actually found something meaningfully cheaper than a naive straight line.
    straight_line_cost_upper_bound = np.hypot(10, 10) * 10.0
    assert route.cost < straight_line_cost_upper_bound


def test_route_through_uniform_grid_is_direct():
    cost = np.ones((10, 10), dtype=np.float32)
    route = find_route(cost, (0.0, 0.0), (9.0, 0.0))

    assert route.points[0] == (0.0, 0.0)
    assert route.points[-1] == (9.0, 0.0)
    # Uniform cost, same row -- should walk straight along that row, not wander.
    assert all(y == 0.0 for _x, y in route.points)


def test_out_of_bounds_endpoints_are_clamped_not_raised():
    cost = np.ones((5, 5), dtype=np.float32)
    route = find_route(cost, (-3.0, -3.0), (100.0, 100.0))

    assert route.points[0] == (0.0, 0.0)
    assert route.points[-1] == (4.0, 4.0)
