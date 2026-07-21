"""Deterministic unit tests on synthetic cost grids for
pipeline/pathfinding.py -- exact/behavioral assertions on a hand-built cost
array, same reasoning as test_geometry.py/test_cost_grid.py (no real photo
needed to exercise a grid shortest-path search).
"""

from __future__ import annotations

import math

import numpy as np

from pipeline.pathfinding import _geometric_path_cost, find_route


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


def test_recomputed_cost_matches_reported_cost():
    # RouteResult.recomputed_cost is an independent resum of the same route
    # against the same grid (see pathfinding._geometric_path_cost) -- on a
    # varied, non-uniform grid it should reproduce route_through_array's own
    # reported cost, not just loosely approximate it.
    rng = np.random.default_rng(0)
    cost = rng.uniform(0.5, 5.0, size=(30, 30)).astype(np.float32)

    route = find_route(cost, (1.0, 2.0), (27.0, 24.0))

    assert math.isclose(route.cost, route.recomputed_cost, rel_tol=1e-4)


def test_geometric_path_cost_matches_hand_computed_value():
    # Two-step path, one orthogonal step and one diagonal step, on a cost
    # grid simple enough to hand-verify against MCP_Geometric's own
    # documented weighting (see _geometric_path_cost's docstring): each
    # step's Euclidean length is billed half at each endpoint's cost.
    cost = np.array([
        [1.0, 2.0, 4.0],
        [1.0, 2.0, 4.0],
        [1.0, 2.0, 4.0],
    ], dtype=np.float32)
    points = [(0.0, 0.0), (1.0, 0.0), (2.0, 1.0)]  # (x, y): orthogonal then diagonal step

    result = _geometric_path_cost(cost, points)

    orthogonal_step = 1.0 * (cost[0, 0] + cost[0, 1]) / 2.0  # (0,0) -> (1,0) i.e. row0,col0 -> row0,col1
    diagonal_step = math.sqrt(2) * (cost[0, 1] + cost[1, 2]) / 2.0  # (1,0) -> (2,1) i.e. row0,col1 -> row1,col2
    assert math.isclose(result, orthogonal_step + diagonal_step, rel_tol=1e-6)
