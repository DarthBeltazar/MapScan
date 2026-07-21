//! Segmentation + course-detection + route results -> a single GeoJSON-like
//! document. Conceptual Rust port of the Python prototype's `vectorize.py`
//! -- see its module docstring: this is the project's internal map format
//! per `prompt.txt` (a GeoJSON-shaped `FeatureCollection` in the rectified
//! image's own local pixel coordinate system, no real-world
//! geo-referencing -- explicitly out of scope). `.omap`/`.ocd` are a
//! separate, unbuilt *input* bypass path, not something this pipeline
//! produces.
//!
//! Returns a plain JSON string rather than a typed struct -- GeoJSON's
//! per-feature `properties` are inherently heterogeneous (a control's
//! `code`, a route's `terrain_breakdown` map, etc.), which maps naturally
//! onto `serde_json::Value` and awkwardly onto a single frb-bridged struct.
//! A `String` also matches how a caller actually wants this (write it
//! straight to a `.geojson` file, same as the Python prototype's
//! `run_pipeline.py`), so no separate parsing step is needed on the Dart
//! side either.

use serde_json::{json, Map, Value};

use super::cost_grid::TerrainFraction;
use super::course_detection::CourseResult;
use super::geometry::{Pt, Segment};
use super::pathfinding::RouteResult;
use super::segmentation::SegmentationResult;

/// Image pixel coords have y growing downward; GeoJSON convention (and every
/// normal map viewer) expects y growing upward -- flip once here rather than
/// carrying image-pixel-flavored coordinates through a file that claims to
/// be GeoJSON.
fn flip_y(y: f32, height: i32) -> f32 {
    height as f32 - y
}

fn point_coords(p: Pt, height: i32) -> Value {
    json!([p.x, flip_y(p.y, height)])
}

fn polygon_geometry(points: &[Pt], height: i32) -> Value {
    let mut ring: Vec<Value> = points.iter().map(|&p| point_coords(p, height)).collect();
    if let Some(&first) = points.first() {
        ring.push(point_coords(first, height)); // GeoJSON polygon rings must be closed.
    }
    json!({"type": "Polygon", "coordinates": [ring]})
}

fn linestring_geometry(points: &[Pt], height: i32) -> Value {
    let coords: Vec<Value> = points.iter().map(|&p| point_coords(p, height)).collect();
    json!({"type": "LineString", "coordinates": coords})
}

fn segment_geometry(seg: &Segment, height: i32) -> Value {
    linestring_geometry(&[seg.a, seg.b], height)
}

fn point_geometry(p: Pt, height: i32) -> Value {
    json!({"type": "Point", "coordinates": [p.x, flip_y(p.y, height)]})
}

fn feature(geometry: Value, properties: Value) -> Value {
    json!({"type": "Feature", "geometry": geometry, "properties": properties})
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_geojson(
    seg: &SegmentationResult,
    course: &CourseResult,
    route: Option<&RouteResult>,
    route_terrain_breakdown: &[TerrainFraction],
    width: i32,
    height: i32,
    scale_to_original: f32,
    mn_line_spacing_px: Option<f32>,
    quad_found: bool,
    source_filename: &str,
) -> String {
    let mut features = Vec::new();

    for class_polys in &seg.polygons_by_class {
        for poly in &class_polys.polygons {
            features.push(feature(
                polygon_geometry(&poly.points, height),
                json!({"terrain_class": class_polys.class_name}),
            ));
        }
    }

    for path_seg in &seg.paths {
        features.push(feature(segment_geometry(path_seg, height), json!({"terrain_class": "path"})));
    }

    for c in &course.controls {
        features.push(feature(
            point_geometry(Pt { x: c.x, y: c.y }, height),
            json!({"role": "control", "code": c.code}),
        ));
    }

    if let Some(start) = course.start {
        features.push(feature(point_geometry(start, height), json!({"role": "start"})));
    }
    if let Some(finish) = course.finish {
        features.push(feature(point_geometry(finish, height), json!({"role": "finish"})));
    }

    for leg in &course.legs {
        features.push(feature(segment_geometry(leg, height), json!({"role": "course_leg"})));
    }

    if let Some(route) = route {
        if route.points.len() >= 2 {
            // "demo_route", not "course_route": there's no control-sequencing
            // graph yet (see `pathfinding.rs`'s module doc), so this is a
            // connectivity proof, not a real full-course route.
            let mut properties = Map::new();
            properties.insert("role".to_string(), json!("demo_route"));
            properties.insert("cost".to_string(), json!(route.cost));
            // Independent cross-check on cost, not a duplicate -- see
            // `RouteResult.recomputed_cost`'s doc comment. Should equal
            // `cost` to within float rounding; a real gap here would mean the
            // grid this route was found on doesn't match the one this
            // FeatureCollection was built from.
            properties.insert("recomputed_cost".to_string(), json!(route.recomputed_cost));
            if !route_terrain_breakdown.is_empty() {
                let mut breakdown = Map::new();
                for f in route_terrain_breakdown {
                    breakdown.insert(f.class_name.clone(), json!(f.fraction));
                }
                properties.insert("terrain_breakdown".to_string(), Value::Object(breakdown));
            }
            features.push(feature(linestring_geometry(&route.points, height), Value::Object(properties)));
        }
    }

    let fc = json!({
        "type": "FeatureCollection",
        "features": features,
        // Non-standard top-level members (RFC 7946 permits foreign members
        // on the FeatureCollection object) -- for a future Flutter renderer
        // to overlay this on the rectified photo without distortion, not
        // meant for generic third-party GeoJSON tools.
        "properties": {
            "source_photo": source_filename,
            "image_width_px": width,
            "image_height_px": height,
            "scale_to_original_photo": scale_to_original,
            "magnetic_north_line_spacing_px": mn_line_spacing_px,
            "quad_rectification_found": quad_found,
        },
    });

    serde_json::to_string(&fc).expect("serde_json::Value serialization is infallible")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::course_detection::Control;
    use crate::api::segmentation::ClassPolygons;

    fn empty_seg() -> SegmentationResult {
        SegmentationResult { polygons_by_class: vec![], paths: vec![] }
    }

    fn empty_course() -> CourseResult {
        CourseResult::default()
    }

    #[test]
    fn flips_y_to_geojson_convention() {
        let seg = empty_seg();
        let mut course = empty_course();
        course.start = Some(Pt { x: 10.0, y: 4.0 });
        let json_str = build_geojson(&seg, &course, None, &[], 100, 100, 1.0, None, true, "test.jpg");
        let v: Value = serde_json::from_str(&json_str).unwrap();
        let features = v["features"].as_array().unwrap();
        let start_feature = features.iter().find(|f| f["properties"]["role"] == "start").unwrap();
        // height=100, y=4 -> flipped y = 96.
        assert_eq!(start_feature["geometry"]["coordinates"], json!([10.0, 96.0]));
    }

    #[test]
    fn polygon_ring_is_closed() {
        let mut seg = empty_seg();
        seg.polygons_by_class.push(ClassPolygons {
            class_name: "forest".to_string(),
            polygons: vec![crate::api::geometry::Polygon {
                points: vec![Pt { x: 0.0, y: 0.0 }, Pt { x: 10.0, y: 0.0 }, Pt { x: 10.0, y: 10.0 }],
            }],
        });
        let json_str = build_geojson(&seg, &empty_course(), None, &[], 100, 100, 1.0, None, true, "test.jpg");
        let v: Value = serde_json::from_str(&json_str).unwrap();
        let coords = v["features"][0]["geometry"]["coordinates"][0].as_array().unwrap();
        assert_eq!(coords.len(), 4, "a 3-point ring must be closed to 4 coordinates in GeoJSON");
        assert_eq!(coords.first(), coords.last());
    }

    #[test]
    fn control_code_is_null_when_unread() {
        let seg = empty_seg();
        let mut course = empty_course();
        course.controls.push(Control { x: 5.0, y: 5.0, radius: 2.0, code: None });
        let json_str = build_geojson(&seg, &course, None, &[], 100, 100, 1.0, None, true, "test.jpg");
        let v: Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(v["features"][0]["properties"]["code"], Value::Null);
    }

    #[test]
    fn omits_route_feature_when_none() {
        let json_str = build_geojson(&empty_seg(), &empty_course(), None, &[], 100, 100, 1.0, None, true, "test.jpg");
        let v: Value = serde_json::from_str(&json_str).unwrap();
        assert!(v["features"].as_array().unwrap().is_empty());
    }

    #[test]
    fn includes_route_with_terrain_breakdown() {
        let route = RouteResult {
            points: vec![Pt { x: 0.0, y: 0.0 }, Pt { x: 10.0, y: 0.0 }],
            cost: 5.0,
            recomputed_cost: 5.0,
        };
        let breakdown = vec![TerrainFraction { class_name: "path".to_string(), fraction: 1.0 }];
        let json_str =
            build_geojson(&empty_seg(), &empty_course(), Some(&route), &breakdown, 100, 100, 1.0, None, true, "test.jpg");
        let v: Value = serde_json::from_str(&json_str).unwrap();
        let f = &v["features"][0];
        assert_eq!(f["properties"]["role"], "demo_route");
        assert_eq!(f["properties"]["terrain_breakdown"]["path"], 1.0);
    }

    #[test]
    fn top_level_properties_carry_photo_metadata() {
        let json_str =
            build_geojson(&empty_seg(), &empty_course(), None, &[], 200, 150, 0.5, Some(12.5), false, "map0.jpg");
        let v: Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(v["type"], "FeatureCollection");
        assert_eq!(v["properties"]["source_photo"], "map0.jpg");
        assert_eq!(v["properties"]["image_width_px"], 200);
        assert_eq!(v["properties"]["image_height_px"], 150);
        assert_eq!(v["properties"]["scale_to_original_photo"], 0.5);
        assert_eq!(v["properties"]["magnetic_north_line_spacing_px"], 12.5);
        assert_eq!(v["properties"]["quad_rectification_found"], false);
    }
}
