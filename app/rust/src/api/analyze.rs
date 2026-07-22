//! Top-level entry point wiring `preprocessing` -> `segmentation` +
//! `course_detection` -> `cost_grid` -> `pathfinding` -> `vectorize` together
//! against one decoded photo, mirroring the Python prototype's
//! `scripts/run_pipeline.py` orchestration end to end.
//!
//! `legend_boxes` and `hough_param2` are caller-supplied, not a hardcoded
//! per-file table baked into this crate -- see `PHASE0_HANDOFF.md`'s
//! explicit guidance that the Python prototype's per-file calibration
//! (`config.LEGEND_EXCLUDE_BOXES`, `config.HOUGH_PARAM2`) doesn't generalize
//! to a photo this repo hasn't seen and belongs behind the manual-correction
//! UI instead of a fixed table.

use opencv::prelude::*;

use super::cost_grid::{self, TerrainFraction};
use super::course_detection::{self, Control, CourseResult};
use super::geometry::Pt;
use super::pathfinding::{self, RouteResult};
use super::segmentation::{self, ExcludeBox, SegmentationResult};
use super::vectorize;

#[derive(Debug, Clone)]
pub struct AnalyzeResult {
    /// Rectified, white-balanced image, PNG-encoded.
    pub image_png: Vec<u8>,
    pub width: i32,
    pub height: i32,
    pub quad_found: bool,
    pub mn_line_xs: Vec<f32>,
    pub mn_line_spacing_px: Option<f32>,
    pub segmentation: SegmentationResult,
    pub course: CourseResult,
    /// Demo route: there's no control-sequencing/course-graph yet
    /// (`course.controls` is an unordered list, matching the Python
    /// prototype's `CourseResult.controls`), so this is "start -> nearest
    /// control", or the first two detected controls if no start was found --
    /// a connectivity proof that the cost grid is pathable, not a real
    /// full-course route. `None` if neither condition is met (see
    /// `pick_demo_route_endpoints`).
    pub route: Option<RouteResult>,
    /// What terrain `route` actually crossed, as a fraction of its length --
    /// empty if `route` is `None`. See `cost_grid::route_terrain_breakdown`.
    pub route_terrain_breakdown: Vec<TerrainFraction>,
    /// This analysis, as a GeoJSON-shaped `FeatureCollection` string -- see
    /// `vectorize::build_geojson`. Write it straight to a `.geojson` file;
    /// there's no separate parsing step needed on the Dart side.
    pub geojson: String,
}

/// Default HoughCircles accumulator threshold if the caller doesn't have a
/// better one yet (e.g. before any manual-correction feedback exists). The
/// Python prototype's equivalent (`HOUGH_PARAM2_DEFAULT = 38`) is itself
/// documented there as an unverified guess, not a validated default -- same
/// caveat applies here.
pub const HOUGH_PARAM2_DEFAULT: f64 = 38.0;

/// Ported as-is from the Python prototype's `config.RING_COVERAGE_MIN_FRACTION`
/// -- this one *is* meant to generalize (it's about how a real printed ring
/// looks, not about one photo's noise level), unlike HOUGH_PARAM2 above.
pub const RING_COVERAGE_MIN_FRACTION_DEFAULT: f32 = 0.75;
pub const START_TRIANGLE_AREA_RATIO_MIN_DEFAULT: f64 = 0.25;
pub const START_TRIANGLE_AREA_RATIO_MAX_DEFAULT: f64 = 1.3;
pub const START_TRIANGLE_MIN_EQUILATERAL_SCORE_DEFAULT: f64 = 0.85;

pub fn analyze_map(
    image_bytes: Vec<u8>,
    legend_boxes: Vec<ExcludeBox>,
    hough_param2: f64,
    source_filename: Option<String>,
) -> Result<AnalyzeResult, String> {
    analyze_map_impl(image_bytes, legend_boxes, hough_param2, source_filename).map_err(|e| e.to_string())
}

fn analyze_map_impl(
    image_bytes: Vec<u8>,
    legend_boxes: Vec<ExcludeBox>,
    hough_param2: f64,
    source_filename: Option<String>,
) -> opencv::Result<AnalyzeResult> {
    let pre = super::preprocessing::preprocess(&image_bytes)?;
    let (width, height) = (pre.rectified.cols(), pre.rectified.rows());

    let base_valid_mask = segmentation::build_valid_mask(width, height, &legend_boxes, None);
    let ink_mask = course_detection::build_course_ink_mask(&pre.rectified, &base_valid_mask)?;

    let course = course_detection::detect_course(
        &ink_mask,
        width,
        height,
        hough_param2,
        RING_COVERAGE_MIN_FRACTION_DEFAULT,
        (START_TRIANGLE_AREA_RATIO_MIN_DEFAULT, START_TRIANGLE_AREA_RATIO_MAX_DEFAULT),
        START_TRIANGLE_MIN_EQUILATERAL_SCORE_DEFAULT,
    )?;

    let terrain_valid_mask = segmentation::build_valid_mask(width, height, &legend_boxes, Some(&ink_mask));
    let segmentation_result = segmentation::segment_terrain(&pre.rectified, &terrain_valid_mask, 6)?;

    // Cost grid uses the *base* valid mask, not the course-ink-excluded one
    // -- course ink drawn over real terrain doesn't make that terrain
    // impassable, only outside the paper itself should cost extra (mirrors
    // Python's run_pipeline.py, which passes its own `valid_mask`, not
    // `seg_mask`, into `build_cost_grid`).
    let cost_grid_result = cost_grid::build_cost_grid(&segmentation_result, width, height, Some(&base_valid_mask))?;

    let (route, route_terrain_breakdown) = match pick_demo_route_endpoints(&course) {
        Some((start, end)) => {
            let route = pathfinding::find_route(&cost_grid_result.cost, width, height, start, end);
            let breakdown =
                cost_grid::route_terrain_breakdown(&cost_grid_result.class_grid, width, height, &route.points);
            (Some(route), breakdown)
        }
        None => (None, vec![]),
    };

    let mut out_buf = opencv::core::Vector::<u8>::new();
    opencv::imgcodecs::imencode_def(".png", &pre.rectified, &mut out_buf)?;

    let geojson = vectorize::build_geojson(
        &segmentation_result,
        &course,
        route.as_ref(),
        &route_terrain_breakdown,
        width,
        height,
        pre.scale_to_original,
        pre.mn_line_spacing_px,
        pre.quad_found,
        source_filename.as_deref().unwrap_or(""),
    );

    Ok(AnalyzeResult {
        image_png: out_buf.to_vec(),
        width,
        height,
        quad_found: pre.quad_found,
        mn_line_xs: pre.mn_line_xs,
        mn_line_spacing_px: pre.mn_line_spacing_px,
        segmentation: segmentation_result,
        course,
        route,
        route_terrain_breakdown,
        geojson,
    })
}

/// Rebuilds the GeoJSON document from a (possibly hand-corrected) copy of
/// `segmentation`/`course` -- the manual-correction UI edits plain Dart-side
/// copies of those structs (add/move/delete a control or start/finish
/// marker, reclassify a terrain polygon's `class_name`), then calls this to
/// get an export that reflects the correction instead of the raw detection.
/// `route`/`route_terrain_breakdown` are passed through unchanged: this pass
/// of the correction UI doesn't recompute the cost grid or route from edited
/// terrain/controls yet (a deliberate scope cut, not an oversight -- see
/// `PHASE0_HANDOFF.md` on control-sequencing being separate follow-up work),
/// so a route already found against the original detection may no longer
/// reflect a moved control; the export is still geometrically the true
/// corrected map, it's only the route/terrain_breakdown properties that can
/// go stale after an edit that affects them.
#[allow(clippy::too_many_arguments)]
pub fn rebuild_geojson(
    segmentation: SegmentationResult,
    course: CourseResult,
    route: Option<RouteResult>,
    route_terrain_breakdown: Vec<TerrainFraction>,
    width: i32,
    height: i32,
    scale_to_original: f32,
    mn_line_spacing_px: Option<f32>,
    quad_found: bool,
    source_filename: String,
) -> String {
    vectorize::build_geojson(
        &segmentation,
        &course,
        route.as_ref(),
        &route_terrain_breakdown,
        width,
        height,
        scale_to_original,
        mn_line_spacing_px,
        quad_found,
        &source_filename,
    )
}

/// Phase 0 has no control-sequencing/course-graph yet (`course_detection`
/// only returns an unordered control list) -- so the demo route this picks
/// is just "start -> nearest control", or the first two detected controls if
/// no start was found. Ported as-is from the Python prototype's
/// `run_pipeline._pick_demo_route_endpoints`.
fn pick_demo_route_endpoints(course: &CourseResult) -> Option<(Pt, Pt)> {
    if let Some(start) = course.start {
        if !course.controls.is_empty() {
            let nearest: &Control = course
                .controls
                .iter()
                .min_by(|a, b| {
                    let da = (a.x - start.x).powi(2) + (a.y - start.y).powi(2);
                    let db = (b.x - start.x).powi(2) + (b.y - start.y).powi(2);
                    da.partial_cmp(&db).unwrap()
                })
                .unwrap();
            return Some((start, Pt { x: nearest.x, y: nearest.y }));
        }
    }
    if course.controls.len() >= 2 {
        let a = &course.controls[0];
        let b = &course.controls[1];
        return Some((Pt { x: a.x, y: a.y }, Pt { x: b.x, y: b.y }));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::geometry::Polygon;
    use crate::api::segmentation::ClassPolygons;

    #[test]
    fn rebuild_geojson_reflects_a_corrected_class_name() {
        let seg = SegmentationResult {
            polygons_by_class: vec![ClassPolygons {
                // A manual correction: a polygon originally detected as
                // "rock" gets reclassified to "forest" by the user.
                class_name: "forest".to_string(),
                polygons: vec![Polygon { points: vec![Pt { x: 0.0, y: 0.0 }, Pt { x: 10.0, y: 0.0 }, Pt { x: 10.0, y: 10.0 }] }],
            }],
            paths: vec![],
        };
        let course = CourseResult::default();
        let json_str = rebuild_geojson(seg, course, None, vec![], 100, 100, 1.0, None, true, "test.jpg".to_string());
        let v: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(v["features"][0]["properties"]["terrain_class"], "forest");
        assert_eq!(v["properties"]["source_photo"], "test.jpg");
    }
}
