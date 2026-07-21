//! Least-cost path between two pixel points on a cost grid.
//!
//! The Python prototype used `skimage.graph.route_through_array`
//! (`MCP_Geometric`, a Dijkstra-family grid shortest-path solver) as a
//! stand-in per `prompt.txt`'s note that Python routing is prototype-only --
//! `PHASE0_HANDOFF.md` explicitly says Rust needs its own implementation,
//! not a wrapped/ported call into Python, so this is a from-scratch 8-connected
//! Dijkstra over the flat cost grid, matching `MCP_Geometric`'s own
//! documented per-step weighting: a step of Euclidean length `d` between two
//! pixels costs `d * (cost[p1] + cost[p2]) / 2` (half the step billed at each
//! endpoint's cost). scikit-fmm/Fast Marching is a later phase, not
//! reimplemented here either.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use super::geometry::Pt;

#[derive(Debug, Clone)]
pub struct RouteResult {
    /// (x, y) pixel coords, image convention (Y-down).
    pub points: Vec<Pt>,
    pub cost: f32,
    /// Independent cross-check on `cost`: a from-scratch resum of the same
    /// route against the same grid using the weighting documented above. If
    /// this drifts from `cost` by more than float rounding, something
    /// upstream is actually wrong, not just "approximate" -- same reasoning
    /// as the Python prototype's `RouteResult.recomputed_cost`.
    pub recomputed_cost: f32,
}

struct HeapItem {
    cost: f32,
    idx: usize,
}

impl PartialEq for HeapItem {
    fn eq(&self, other: &Self) -> bool {
        self.cost == other.cost
    }
}
impl Eq for HeapItem {}
impl PartialOrd for HeapItem {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for HeapItem {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reversed so `BinaryHeap` (a max-heap) pops the smallest cost first.
        other.cost.partial_cmp(&self.cost).unwrap_or(Ordering::Equal)
    }
}

const NEIGHBORS: [(i32, i32, f32); 8] = [
    (-1, 0, 1.0),
    (1, 0, 1.0),
    (0, -1, 1.0),
    (0, 1, 1.0),
    (-1, -1, std::f32::consts::SQRT_2),
    (-1, 1, std::f32::consts::SQRT_2),
    (1, -1, std::f32::consts::SQRT_2),
    (1, 1, std::f32::consts::SQRT_2),
];

fn xy_to_rc(pt: Pt, width: i32, height: i32) -> (i32, i32) {
    let row = (pt.y.round() as i32).clamp(0, height - 1);
    let col = (pt.x.round() as i32).clamp(0, width - 1);
    (row, col)
}

fn geometric_path_cost(cost: &[f32], width: i32, points: &[Pt]) -> f32 {
    let mut total = 0.0f32;
    for pair in points.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let (r1, c1) = (a.y.round() as i32, a.x.round() as i32);
        let (r2, c2) = (b.y.round() as i32, b.x.round() as i32);
        let dist = ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt();
        let c_a = cost[(r1 * width + c1) as usize];
        let c_b = cost[(r2 * width + c2) as usize];
        total += dist * (c_a + c_b) / 2.0;
    }
    total
}

/// Out-of-bounds endpoints are clamped into the grid, not rejected --
/// rectification slop can easily put a detected control a pixel or two
/// outside the working image, and failing outright there would be a worse
/// outcome than a clamped, still-useful route.
pub(crate) fn find_route(cost: &[f32], width: i32, height: i32, start_xy: Pt, end_xy: Pt) -> RouteResult {
    let (sr, sc) = xy_to_rc(start_xy, width, height);
    let (er, ec) = xy_to_rc(end_xy, width, height);
    let n = (width * height) as usize;
    let start_idx = (sr * width + sc) as usize;
    let end_idx = (er * width + ec) as usize;

    let mut dist = vec![f32::INFINITY; n];
    let mut prev = vec![u32::MAX; n];
    dist[start_idx] = 0.0;
    let mut heap = BinaryHeap::new();
    heap.push(HeapItem { cost: 0.0, idx: start_idx });

    while let Some(HeapItem { cost: d, idx }) = heap.pop() {
        if d > dist[idx] {
            continue;
        }
        if idx == end_idx {
            break;
        }
        let r = (idx as i32) / width;
        let c = (idx as i32) % width;
        for &(dr, dc, step_len) in &NEIGHBORS {
            let (nr, nc) = (r + dr, c + dc);
            if nr < 0 || nr >= height || nc < 0 || nc >= width {
                continue;
            }
            let nidx = (nr * width + nc) as usize;
            let step_cost = step_len * (cost[idx] + cost[nidx]) / 2.0;
            let nd = d + step_cost;
            if nd < dist[nidx] {
                dist[nidx] = nd;
                prev[nidx] = idx as u32;
                heap.push(HeapItem { cost: nd, idx: nidx });
            }
        }
    }

    let mut path_indices = vec![end_idx];
    let mut cur = end_idx;
    while cur != start_idx {
        let p = prev[cur];
        if p == u32::MAX {
            break; // unreachable -- shouldn't happen on a finite-cost fully-connected grid
        }
        cur = p as usize;
        path_indices.push(cur);
    }
    path_indices.reverse();

    let points: Vec<Pt> = path_indices
        .iter()
        .map(|&idx| Pt { x: ((idx as i32) % width) as f32, y: ((idx as i32) / width) as f32 })
        .collect();

    let cost_reported = dist[end_idx];
    let recomputed_cost = geometric_path_cost(cost, width, &points);

    RouteResult { points, cost: cost_reported, recomputed_cost }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_prefers_cheap_corridor_over_expensive_wall() {
        // A cheap horizontal corridor at row 5 through an otherwise
        // expensive field; start/end sit off that row, so the router has to
        // detour onto it rather than cutting straight across the wall.
        let (w, h) = (11, 11);
        let mut cost = vec![10.0f32; (w * h) as usize];
        for x in 0..w {
            cost[(5 * w + x) as usize] = 1.0;
        }
        let route = find_route(&cost, w, h, Pt { x: 0.0, y: 0.0 }, Pt { x: 10.0, y: 10.0 });

        assert!(route.points.iter().any(|p| p.y.round() as i32 == 5));
        let straight_line_upper_bound = (10.0f32 * 10.0 + 10.0 * 10.0).sqrt() * 10.0;
        assert!(route.cost < straight_line_upper_bound);
    }

    #[test]
    fn route_through_uniform_grid_is_direct() {
        let (w, h) = (10, 10);
        let cost = vec![1.0f32; (w * h) as usize];
        let route = find_route(&cost, w, h, Pt { x: 0.0, y: 0.0 }, Pt { x: 9.0, y: 0.0 });

        assert_eq!((route.points[0].x, route.points[0].y), (0.0, 0.0));
        assert_eq!((route.points.last().unwrap().x, route.points.last().unwrap().y), (9.0, 0.0));
        assert!(route.points.iter().all(|p| p.y == 0.0));
    }

    #[test]
    fn out_of_bounds_endpoints_are_clamped_not_panicking() {
        let (w, h) = (5, 5);
        let cost = vec![1.0f32; (w * h) as usize];
        let route = find_route(&cost, w, h, Pt { x: -3.0, y: -3.0 }, Pt { x: 100.0, y: 100.0 });

        assert_eq!((route.points[0].x, route.points[0].y), (0.0, 0.0));
        assert_eq!((route.points.last().unwrap().x, route.points.last().unwrap().y), (4.0, 4.0));
    }

    #[test]
    fn recomputed_cost_matches_reported_cost() {
        // Simple deterministic pseudo-random cost field (no external RNG
        // dependency needed for a smoke check like this).
        let (w, h) = (30, 30);
        let mut cost = vec![0.0f32; (w * h) as usize];
        let mut seed: u32 = 12345;
        for c in cost.iter_mut() {
            seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
            let frac = ((seed >> 8) & 0xFFFF) as f32 / 65535.0;
            *c = 0.5 + frac * 4.5;
        }
        let route = find_route(&cost, w, h, Pt { x: 1.0, y: 2.0 }, Pt { x: 27.0, y: 24.0 });
        assert!((route.cost - route.recomputed_cost).abs() < route.cost.max(1.0) * 1e-3);
    }

    #[test]
    fn geometric_path_cost_matches_hand_computed_value() {
        // Two-step path, one orthogonal step and one diagonal step, on a
        // cost grid simple enough to hand-verify: each step's Euclidean
        // length is billed half at each endpoint's cost.
        let (w, _h) = (3, 3);
        let cost = vec![1.0, 2.0, 4.0, 1.0, 2.0, 4.0, 1.0, 2.0, 4.0];
        let points = vec![Pt { x: 0.0, y: 0.0 }, Pt { x: 1.0, y: 0.0 }, Pt { x: 2.0, y: 1.0 }];

        let result = geometric_path_cost(&cost, w, &points);

        let orthogonal_step = 1.0 * (cost[0] + cost[1]) / 2.0;
        let diagonal_step = (2.0f32).sqrt() * (cost[1] + cost[3 + 2]) / 2.0;
        assert!((result - (orthogonal_step + diagonal_step)).abs() < 1e-6);
    }
}
