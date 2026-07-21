//! Runs `analyze_map` (segmentation + course detection) against the same
//! real in-scope photos as `rectify_photo.rs` and the Python prototype's
//! `test_pipeline_golden.py` -- invariant checks (sane polygon/path counts,
//! control count within a tolerance band of the Python prototype's
//! manually-counted baselines), not exact-match, for the same reason golden
//! test uses: no pixel-level ground truth exists for these photos.
//!
//! `legend_boxes`/`hough_param2` below are ported from
//! `python_prototype/pipeline/config.py`'s `LEGEND_EXCLUDE_BOXES`/
//! `HOUGH_PARAM2` *for this test only* -- they calibrate one specific
//! rectified photo's known layout and are not baked into `rust_core` itself
//! (see `analyze_map`'s doc comment and `PHASE0_HANDOFF.md`). A real caller
//! (the eventual manual-correction UI) supplies its own values per photo;
//! this test reuses the Python prototype's already-verified ones purely to
//! exercise the Rust port against a known-working calibration.

use std::path::PathBuf;

use rust_core::api::analyze::analyze_map;
use rust_core::api::segmentation::ExcludeBox;

fn eb(x0: f32, y0: f32, x1: f32, y1: f32) -> ExcludeBox {
    ExcludeBox { x0, y0, x1, y1 }
}

fn test_data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("python_prototype")
        .join("testData")
}

struct Case {
    filename: &'static str,
    legend_boxes: Vec<ExcludeBox>,
    hough_param2: f64,
    manual_control_count: usize,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            filename: "map0.jpg",
            legend_boxes: vec![
                eb(0.0, 0.0, 1.0, 0.09),
                eb(0.625, 0.0, 1.0, 0.38),
                eb(0.44, 0.10, 0.61, 0.28),
                eb(0.245, 0.108, 0.36, 0.28),
                eb(0.07, 0.31, 0.16, 0.445),
                eb(0.0275, 0.466, 0.13, 0.552),
                eb(0.345, 0.402, 0.435, 0.574),
                eb(0.07, 0.61, 0.195, 0.775),
                eb(0.4375, 0.323, 0.645, 0.402),
                eb(0.645, 0.38, 0.81, 0.43),
                eb(0.0075, 0.854, 0.08, 0.965),
                eb(0.915, 0.861, 0.995, 0.954),
            ],
            hough_param2: 40.0,
            manual_control_count: 18,
        },
        Case {
            filename: "map2.jpg",
            legend_boxes: vec![eb(0.0, 0.0, 1.0, 0.05), eb(0.0, 0.0, 0.40, 0.18), eb(0.58, 0.48, 1.0, 0.91)],
            hough_param2: 44.0,
            manual_control_count: 9,
        },
        Case {
            filename: "map4.jpg",
            // Re-derived directly against this Rust pipeline's own
            // (unrotated) rectified image -- the Python prototype's
            // equivalent boxes (badge top-left, title bottom-right) are
            // calibrated against `config.PAGE_ROTATION_K`'s 270-degree
            // rotation of this file, which this pipeline deliberately does
            // NOT apply (see `preprocessing.rs`), so those coordinates land
            // in the wrong place here. Checked directly by rendering
            // `rectify_photo`'s plain output with a fractional grid overlay:
            // in this pipeline's frame, the title block is top-right and the
            // "75 ЛЕТ ФИЗТЕХ"/КВАРК/МФТИ logo cluster is bottom-left --
            // rotated roughly 180 degrees from Python's layout, consistent
            // with the two pipelines' rotation conventions actually
            // differing by that much for this file.
            legend_boxes: vec![eb(0.49, 0.0, 0.97, 0.43), eb(0.0, 0.60, 0.30, 0.96)],
            hough_param2: 46.0,
            manual_control_count: 17,
        },
        Case {
            filename: "map6.jpg",
            // Re-derived the same way as map4.jpg above, and for the same
            // reason (this file also needs a 270-degree rotation in
            // `config.PAGE_ROTATION_K` that this Rust pipeline doesn't
            // apply) -- title top-right, logo cluster bottom-left in this
            // pipeline's actual (unrotated) frame.
            legend_boxes: vec![eb(0.49, 0.0, 0.78, 0.33), eb(0.0, 0.68, 0.29, 0.97)],
            hough_param2: 42.0,
            manual_control_count: 9,
        },
    ]
}

#[test]
fn analyze_in_scope_photos() {
    for case in cases() {
        let path = test_data_dir().join(case.filename);
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("failed to read {path:?}: {e}"));

        let result = analyze_map(bytes, case.legend_boxes, case.hough_param2, Some(case.filename.to_string()))
            .unwrap_or_else(|e| panic!("analyze_map failed on {}: {e}", case.filename));

        assert!(result.width > 0 && result.height > 0, "{}: empty output image", case.filename);
        let total_area = (result.width as f64) * (result.height as f64);

        let mut class_names: Vec<&str> =
            result.segmentation.polygons_by_class.iter().map(|c| c.class_name.as_str()).collect();
        class_names.sort();
        assert_eq!(
            class_names,
            vec!["clearing", "forest", "marsh", "out_of_bounds", "rock", "thicket", "water"],
            "{}: unexpected terrain class set",
            case.filename
        );

        let segmented_area: f64 = result
            .segmentation
            .polygons_by_class
            .iter()
            .flat_map(|c| &c.polygons)
            .map(|p| polygon_area(&p.points))
            .sum();
        // Draft segmentation never covers the whole valid area (legend
        // exclusion, gaps between k-means clusters discarded as noise,
        // course ink carved out) -- same "plausible fraction, not exact"
        // invariant as the Python prototype's test_pipeline_golden.py.
        assert!(
            segmented_area > 0.05 * total_area,
            "{}: segmented area {:.0} implausibly small vs. total {:.0}",
            case.filename,
            segmented_area,
            total_area
        );
        assert!(
            segmented_area < total_area,
            "{}: segmented area {:.0} exceeds total {:.0}",
            case.filename,
            segmented_area,
            total_area
        );

        // Control count within a wide tolerance band of the Python
        // prototype's one-observer manual count -- these are draft
        // detections on real noisy photos, not expected to match exactly
        // (see MANUAL_KP_COUNTS's docstring in config.py), and this Rust
        // port additionally has no OCR/finish-dedup parity guarantee with
        // the exact Python run it was calibrated against.
        let found = result.course.controls.len();
        let manual = case.manual_control_count;
        let tolerance = (manual / 2).max(3);
        assert!(
            found + tolerance >= manual && found <= manual + tolerance,
            "{}: found {found} controls, manual baseline {manual} (tolerance {tolerance})",
            case.filename
        );

        // Every in-scope file has either a start + >=1 control, or >=2
        // controls (checked directly against this test's own printed
        // output), so a demo route should always be found here -- same
        // "connectivity proof, not a real course route" invariant as the
        // Python prototype's test_pipeline_golden.py.
        let route = result.route.as_ref().unwrap_or_else(|| panic!("{}: no demo route found", case.filename));
        assert!(route.points.len() >= 2, "{}: demo route has too few points", case.filename);
        for p in &route.points {
            assert!(
                p.x >= 0.0 && p.x < result.width as f32 && p.y >= 0.0 && p.y < result.height as f32,
                "{}: route point ({}, {}) out of bounds",
                case.filename,
                p.x,
                p.y
            );
        }
        // recomputed_cost is a from-scratch resum of this same route against
        // this same grid (see pathfinding::RouteResult's doc comment) --
        // asserting it here, not just at the synthetic-grid unit-test level,
        // catches any real-grid-specific mismatch (e.g. an indexing bug that
        // only shows up on a non-square grid).
        assert!(
            (route.cost - route.recomputed_cost).abs() < route.cost.max(1.0) * 1e-2,
            "{}: route cost {} vs recomputed {} mismatch",
            case.filename,
            route.cost,
            route.recomputed_cost
        );
        assert!(
            (result.route_terrain_breakdown.iter().map(|f| f.fraction).sum::<f32>() - 1.0).abs() < 1e-3,
            "{}: route terrain breakdown fractions don't sum to 1",
            case.filename
        );

        // vectorize::build_geojson's output -- same light-touch invariant
        // check as the Python prototype's test_smoke.py (parses as valid
        // JSON, is a FeatureCollection, non-empty), plus a couple of
        // cross-checks against this same result's other fields that Python's
        // test doesn't make (source_photo matches, feature count is at least
        // the polygon+path+control+leg+route count, since some classes may
        // contribute zero).
        let geojson: serde_json::Value =
            serde_json::from_str(&result.geojson).unwrap_or_else(|e| panic!("{}: invalid GeoJSON: {e}", case.filename));
        assert_eq!(geojson["type"], "FeatureCollection", "{}: not a FeatureCollection", case.filename);
        assert_eq!(geojson["properties"]["source_photo"], case.filename, "{}: source_photo mismatch", case.filename);
        assert_eq!(geojson["properties"]["image_width_px"], result.width, "{}: width mismatch", case.filename);
        let features = geojson["features"].as_array().unwrap_or_else(|| panic!("{}: features not an array", case.filename));
        let min_expected_features = result.segmentation.paths.len()
            + result.course.controls.len()
            + result.course.legs.len()
            + result.segmentation.polygons_by_class.iter().map(|c| c.polygons.len()).sum::<usize>();
        assert!(
            features.len() >= min_expected_features,
            "{}: expected at least {min_expected_features} features, got {}",
            case.filename,
            features.len()
        );

        let breakdown_str: Vec<String> =
            result.route_terrain_breakdown.iter().map(|f| format!("{}={:.0}%", f.class_name, f.fraction * 100.0)).collect();
        println!(
            "{}: controls={found} (manual~{manual}) legs={} start={:?} finish={:?} paths={} segmented_area_frac={:.2} \
             route_points={} route_cost={:.1} terrain=[{}]",
            case.filename,
            result.course.legs.len(),
            result.course.start.is_some(),
            result.course.finish.is_some(),
            result.segmentation.paths.len(),
            segmented_area / total_area,
            route.points.len(),
            route.cost,
            breakdown_str.join(", "),
        );
    }
}

fn polygon_area(points: &[rust_core::api::geometry::Pt]) -> f64 {
    // Shoelace formula -- points are a simplified pixel-contour ring.
    if points.len() < 3 {
        return 0.0;
    }
    let mut sum = 0.0f64;
    for i in 0..points.len() {
        let a = points[i];
        let b = points[(i + 1) % points.len()];
        sum += (a.x as f64) * (b.y as f64) - (b.x as f64) * (a.y as f64);
    }
    (sum / 2.0).abs()
}
