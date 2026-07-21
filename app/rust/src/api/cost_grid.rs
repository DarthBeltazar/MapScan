//! Terrain polygons + path lines -> a per-pixel traversal-cost grid.
//! Conceptual Rust port of the Phase 0 Python prototype's `cost_grid.py`.
//! Rasterizes `segmentation::SegmentationResult` into a flat cost array in
//! the same pixel coordinate system as the working-resolution image (Y-down,
//! matching `segmentation`/`course_detection` output).
//!
//! `TERRAIN_COST` below is the one piece of Python's `config.py` calibration
//! that *is* meant to generalize (see `PHASE0_HANDOFF.md`): it's a
//! qualitative ISOM-convention ordering (path cheapest, then clearing,
//! forest, rock, marsh, thicket, with water/out-of-bounds strongly avoided),
//! not a measured running-speed model -- no field data exists to calibrate
//! real speeds against, unlike `LEGEND_EXCLUDE_BOXES`/`HOUGH_PARAM2` which
//! are photo-specific and deliberately NOT hardcoded here.

use std::collections::HashMap;

use opencv::core::{self, Mat, Point, Scalar, Vector};
use opencv::imgproc;

use super::geometry::{Polygon, Pt, Segment};
use super::mask_utils::mat_to_mask;
use super::segmentation::SegmentationResult;

pub(crate) const TERRAIN_COST_PATH: f32 = 0.8;
pub(crate) const TERRAIN_COST_CLEARING: f32 = 1.0;
pub(crate) const TERRAIN_COST_FOREST: f32 = 1.5;
pub(crate) const TERRAIN_COST_ROCK: f32 = 2.5;
pub(crate) const TERRAIN_COST_MARSH: f32 = 3.0;
pub(crate) const TERRAIN_COST_THICKET: f32 = 4.0;
pub(crate) const TERRAIN_COST_WATER: f32 = 20.0;
pub(crate) const TERRAIN_COST_OUT_OF_BOUNDS: f32 = 50.0;

/// Cost for pixels not covered by any detected terrain polygon (gaps between
/// polygons, k-means clusters discarded as noise) -- treated as neutral
/// ("clearing"-like) rather than a barrier, since these gaps are a
/// segmentation artifact, not real terrain.
pub(crate) const DEFAULT_TERRAIN_COST: f32 = TERRAIN_COST_CLEARING;
/// Cost for pixels outside the valid (paper) mask entirely. Kept finite (not
/// infinite) so a start/end point landing just outside the valid mask from
/// rectification slop doesn't make path-finding fail outright.
pub(crate) const OUTSIDE_VALID_MASK_COST: f32 = TERRAIN_COST_OUT_OF_BOUNDS;
/// Segmentation's path `Segment`s are zero-width centerlines; this is how
/// many pixels wide to draw them into the cost grid so they register as
/// cheap terrain rather than being lost between grid cells.
pub(crate) const PATH_COST_LINE_WIDTH_PX: i32 = 3;

fn terrain_cost(class_name: &str) -> f32 {
    match class_name {
        "path" => TERRAIN_COST_PATH,
        "clearing" => TERRAIN_COST_CLEARING,
        "forest" => TERRAIN_COST_FOREST,
        "rock" => TERRAIN_COST_ROCK,
        "marsh" => TERRAIN_COST_MARSH,
        "thicket" => TERRAIN_COST_THICKET,
        "water" => TERRAIN_COST_WATER,
        "out_of_bounds" => TERRAIN_COST_OUT_OF_BOUNDS,
        _ => DEFAULT_TERRAIN_COST,
    }
}

/// Draw order for area classes: later entries paint over earlier ones, so if
/// polygons from different classes ever overlap (segmentation doesn't
/// guarantee disjointness), the stricter/more-expensive class wins rather
/// than the cheaper one -- the safer failure mode for a "don't run through
/// the lake" tool.
const AREA_DRAW_ORDER: [&str; 7] = ["clearing", "forest", "thicket", "rock", "marsh", "water", "out_of_bounds"];

pub(crate) const CLASS_DEFAULT_GAP: &str = "default_gap";
pub(crate) const CLASS_OUTSIDE_VALID_MASK: &str = "outside_valid_mask";

/// Parallel class labels for `CostGridResult.class_grid`, in the same paint
/// order as `build_cost_grid` itself (each later entry overwrites earlier
/// ones at the same pixel, same as the cost array).
fn class_names() -> Vec<&'static str> {
    let mut v = vec![CLASS_DEFAULT_GAP];
    v.extend_from_slice(&AREA_DRAW_ORDER);
    v.push("path");
    v.push(CLASS_OUTSIDE_VALID_MASK);
    v
}

pub(crate) struct CostGridResult {
    pub cost: Vec<f32>,
    /// Which paint layer actually set each pixel's cost (index into
    /// `class_names()`) -- lets `route_terrain_breakdown` answer "what
    /// terrain did this route actually cross" without reverse-engineering it
    /// from cost values alone, which collide across classes on purpose (e.g.
    /// `default_gap` and `clearing` share a cost).
    pub class_grid: Vec<u8>,
}

fn fill_polygons_mask(polys: &[Polygon], width: i32, height: i32) -> opencv::Result<Vec<u8>> {
    let mut mat = Mat::new_rows_cols_with_default(height, width, core::CV_8U, Scalar::all(0.0))?;
    let mut contours = Vector::<Vector<Point>>::new();
    for poly in polys {
        let pts: Vec<Point> = poly.points.iter().map(|p| Point::new(p.x.round() as i32, p.y.round() as i32)).collect();
        contours.push(Vector::from_slice(&pts));
    }
    imgproc::fill_poly_def(&mut mat, &contours, Scalar::all(1.0))?;
    mat_to_mask(&mat)
}

fn draw_lines_mask(segments: &[Segment], width: i32, height: i32) -> opencv::Result<Vec<u8>> {
    let mut mat = Mat::new_rows_cols_with_default(height, width, core::CV_8U, Scalar::all(0.0))?;
    for seg in segments {
        let pts =
            [Point::new(seg.a.x.round() as i32, seg.a.y.round() as i32), Point::new(seg.b.x.round() as i32, seg.b.y.round() as i32)];
        let mut contours = Vector::<Vector<Point>>::new();
        contours.push(Vector::from_slice(&pts));
        imgproc::polylines(&mut mat, &contours, false, Scalar::all(1.0), PATH_COST_LINE_WIDTH_PX, imgproc::LINE_8, 0)?;
    }
    mat_to_mask(&mat)
}

pub(crate) fn build_cost_grid(
    seg: &SegmentationResult,
    width: i32,
    height: i32,
    valid_mask: Option<&[u8]>,
) -> opencv::Result<CostGridResult> {
    let names = class_names();
    let n = (width * height) as usize;
    let mut cost = vec![DEFAULT_TERRAIN_COST; n];
    let mut class_grid = vec![0u8; n]; // 0 == CLASS_DEFAULT_GAP, first entry of class_names()

    for cls in AREA_DRAW_ORDER {
        let Some(class_polys) = seg.polygons_by_class.iter().find(|c| c.class_name == cls) else { continue };
        if class_polys.polygons.is_empty() {
            continue;
        }
        let mask = fill_polygons_mask(&class_polys.polygons, width, height)?;
        let code = names.iter().position(|&n| n == cls).unwrap() as u8;
        let c = terrain_cost(cls);
        for i in 0..n {
            if mask[i] != 0 {
                cost[i] = c;
                class_grid[i] = code;
            }
        }
    }

    if !seg.paths.is_empty() {
        let mask = draw_lines_mask(&seg.paths, width, height)?;
        let code = names.iter().position(|&n| n == "path").unwrap() as u8;
        for i in 0..n {
            if mask[i] != 0 {
                cost[i] = TERRAIN_COST_PATH;
                class_grid[i] = code;
            }
        }
    }

    if let Some(valid) = valid_mask {
        let code = names.iter().position(|&n| n == CLASS_OUTSIDE_VALID_MASK).unwrap() as u8;
        for i in 0..n {
            if valid[i] == 0 {
                cost[i] = OUTSIDE_VALID_MASK_COST;
                class_grid[i] = code;
            }
        }
    }

    Ok(CostGridResult { cost, class_grid })
}

#[derive(Debug, Clone)]
pub struct TerrainFraction {
    pub class_name: String,
    pub fraction: f32,
}

/// What terrain a route (a list of pixel points) actually crossed, as a
/// fraction of its total geometric length per terrain class. Each step's
/// length is attributed to the terrain class at its midpoint pixel,
/// consistent with how `pathfinding`'s Dijkstra port treats a step as
/// spanning both endpoint pixels. Ported as-is from the Python prototype's
/// `route_terrain_breakdown` -- see its docstring for why this, not the raw
/// route cost number, is the human-legible correctness check ("a low-cost
/// route is only reassuring if it's low *because* it's mostly on path/
/// clearing, not because it cut a chord through an under-detected gap").
pub(crate) fn route_terrain_breakdown(class_grid: &[u8], width: i32, height: i32, points: &[Pt]) -> Vec<TerrainFraction> {
    let names = class_names();
    let mut totals: HashMap<&str, f32> = HashMap::new();
    for pair in points.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let dist = ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt();
        if dist == 0.0 {
            continue;
        }
        let mx = (((a.x + b.x) / 2.0).round() as i32).clamp(0, width - 1);
        let my = (((a.y + b.y) / 2.0).round() as i32).clamp(0, height - 1);
        let code = class_grid[(my * width + mx) as usize];
        let name = names[code as usize];
        *totals.entry(name).or_insert(0.0) += dist;
    }
    let total_len: f32 = totals.values().sum();
    if total_len <= 0.0 {
        return vec![];
    }
    let mut out: Vec<TerrainFraction> =
        totals.into_iter().map(|(name, len)| TerrainFraction { class_name: name.to_string(), fraction: len / total_len }).collect();
    out.sort_by(|a, b| b.fraction.partial_cmp(&a.fraction).unwrap());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::segmentation::ClassPolygons;

    fn empty_seg() -> SegmentationResult {
        SegmentationResult { polygons_by_class: vec![], paths: vec![] }
    }

    fn square_polygon(x0: f32, y0: f32, x1: f32, y1: f32) -> Polygon {
        Polygon { points: vec![Pt { x: x0, y: y0 }, Pt { x: x0, y: y1 }, Pt { x: x1, y: y1 }, Pt { x: x1, y: y0 }] }
    }

    #[test]
    fn default_cost_fills_ungrouped_background() {
        let result = build_cost_grid(&empty_seg(), 20, 20, None).unwrap();
        assert_eq!(result.cost.len(), 400);
        assert!(result.cost.iter().all(|&c| c == DEFAULT_TERRAIN_COST));
    }

    #[test]
    fn area_class_paints_its_terrain_cost() {
        let mut seg = empty_seg();
        seg.polygons_by_class.push(ClassPolygons {
            class_name: "thicket".to_string(),
            polygons: vec![square_polygon(2.0, 2.0, 8.0, 8.0)],
        });
        let result = build_cost_grid(&seg, 10, 10, None).unwrap();
        assert_eq!(result.cost[5 * 10 + 5], TERRAIN_COST_THICKET);
        assert_eq!(result.cost[0], DEFAULT_TERRAIN_COST);
    }

    #[test]
    fn overlapping_area_classes_stricter_class_wins() {
        let mut seg = empty_seg();
        seg.polygons_by_class.push(ClassPolygons {
            class_name: "clearing".to_string(),
            polygons: vec![square_polygon(0.0, 0.0, 10.0, 10.0)],
        });
        seg.polygons_by_class.push(ClassPolygons {
            class_name: "water".to_string(),
            polygons: vec![square_polygon(3.0, 3.0, 7.0, 7.0)],
        });
        let result = build_cost_grid(&seg, 10, 10, None).unwrap();
        assert_eq!(result.cost[5 * 10 + 5], TERRAIN_COST_WATER);
        assert_eq!(result.cost[1 * 10 + 1], TERRAIN_COST_CLEARING);
    }

    #[test]
    fn path_line_is_drawn_cheap_over_area_class() {
        let mut seg = empty_seg();
        seg.polygons_by_class.push(ClassPolygons {
            class_name: "forest".to_string(),
            polygons: vec![square_polygon(0.0, 0.0, 20.0, 20.0)],
        });
        seg.paths.push(Segment { a: Pt { x: 0.0, y: 10.0 }, b: Pt { x: 19.0, y: 10.0 } });
        let result = build_cost_grid(&seg, 20, 20, None).unwrap();
        assert_eq!(result.cost[10 * 20 + 10], TERRAIN_COST_PATH);
        assert_eq!(result.cost[1 * 20 + 1], TERRAIN_COST_FOREST);
    }

    #[test]
    fn valid_mask_excludes_outside_area() {
        let seg = empty_seg();
        let mut valid = vec![0u8; 100];
        for y in 2..8 {
            for x in 2..8 {
                valid[y * 10 + x] = 255;
            }
        }
        let result = build_cost_grid(&seg, 10, 10, Some(&valid)).unwrap();
        assert_eq!(result.cost[0], OUTSIDE_VALID_MASK_COST);
        assert_eq!(result.cost[5 * 10 + 5], DEFAULT_TERRAIN_COST);
    }

    #[test]
    fn class_grid_labels_match_the_cost_grid_they_explain() {
        let mut seg = empty_seg();
        seg.polygons_by_class.push(ClassPolygons {
            class_name: "clearing".to_string(),
            polygons: vec![square_polygon(0.0, 0.0, 10.0, 10.0)],
        });
        seg.polygons_by_class.push(ClassPolygons {
            class_name: "water".to_string(),
            polygons: vec![square_polygon(3.0, 3.0, 7.0, 7.0)],
        });
        seg.paths.push(Segment { a: Pt { x: 0.0, y: 1.0 }, b: Pt { x: 9.0, y: 1.0 } });
        let mut valid = vec![255u8; 100];
        for x in 0..10 {
            valid[9 * 10 + x] = 0;
        }
        let result = build_cost_grid(&seg, 10, 10, Some(&valid)).unwrap();
        let names = class_names();
        assert_eq!(names[result.class_grid[5 * 10 + 5] as usize], "water");
        assert_eq!(names[result.class_grid[8 * 10 + 8] as usize], "clearing");
        assert_eq!(names[result.class_grid[1 * 10 + 8] as usize], "path");
        assert_eq!(names[result.class_grid[9 * 10 + 0] as usize], "outside_valid_mask");
    }

    #[test]
    fn route_terrain_breakdown_reflects_where_the_route_actually_went() {
        let mut seg = empty_seg();
        seg.polygons_by_class.push(ClassPolygons {
            class_name: "forest".to_string(),
            polygons: vec![square_polygon(0.0, 0.0, 20.0, 20.0)],
        });
        seg.paths.push(Segment { a: Pt { x: 0.0, y: 10.0 }, b: Pt { x: 19.0, y: 10.0 } });
        let result = build_cost_grid(&seg, 20, 20, None).unwrap();

        let points = vec![Pt { x: 0.0, y: 10.0 }, Pt { x: 15.0, y: 10.0 }, Pt { x: 15.0, y: 19.0 }];
        let breakdown = route_terrain_breakdown(&result.class_grid, 20, 20, &points);
        let names: Vec<&str> = breakdown.iter().map(|f| f.class_name.as_str()).collect();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"path"));
        assert!(names.contains(&"forest"));
        let path_frac = breakdown.iter().find(|f| f.class_name == "path").unwrap().fraction;
        let forest_frac = breakdown.iter().find(|f| f.class_name == "forest").unwrap().fraction;
        assert!(path_frac > forest_frac);
        let sum: f32 = breakdown.iter().map(|f| f.fraction).sum();
        assert!((sum - 1.0).abs() < 1e-6);
    }

    #[test]
    fn route_terrain_breakdown_empty_for_degenerate_route() {
        let class_grid = vec![0u8; 25];
        let breakdown = route_terrain_breakdown(&class_grid, 5, 5, &[Pt { x: 1.0, y: 1.0 }]);
        assert!(breakdown.is_empty());
    }
}
