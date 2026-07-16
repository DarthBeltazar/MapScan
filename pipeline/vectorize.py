"""Segmentation + course-detection results -> a single GeoJSON-like document.

Internal map format per the project architecture (prompt.txt): a
GeoJSON-shaped FeatureCollection, in the rectified image's own local pixel
coordinate system (no real-world geo-referencing -- decided out of scope for
this task). `.omap`/`.ocd` are a separate, unbuilt *input* bypass path, not
something this pipeline produces -- see plan.
"""

from __future__ import annotations

from shapely.geometry import mapping
from shapely.ops import transform as shapely_transform

from pipeline.config import TERRAIN_CLASSES
from pipeline.course_detection import CourseResult
from pipeline.preprocessing import PreprocessResult
from pipeline.segmentation import SegmentationResult


def _flip_y(height: int):
    """Image pixel coords have y growing downward; GeoJSON convention
    (and every normal map viewer) expects y growing upward. Flip once here
    rather than carrying image-pixel-flavoured coordinates through a file
    that claims to be GeoJSON."""
    def _f(x, y, z=None):
        return (x, height - y)
    return _f


def _geom_feature(geom, height: int, properties: dict) -> dict:
    flipped = shapely_transform(_flip_y(height), geom)
    return {"type": "Feature", "geometry": mapping(flipped), "properties": properties}


def build_feature_collection(
    seg: SegmentationResult,
    course: CourseResult,
    preprocess: PreprocessResult,
    source_filename: str,
) -> dict:
    height = preprocess.image.shape[0]
    width = preprocess.image.shape[1]
    features = []

    for cls in TERRAIN_CLASSES:
        if cls == "path":
            continue  # linear, handled below alongside course legs
        for poly in seg.polygons.get(cls, []):
            features.append(_geom_feature(poly, height, {"terrain_class": cls}))

    for line in seg.paths:
        features.append(_geom_feature(line, height, {"terrain_class": "path"}))

    for c in course.controls:
        point = _point_geom(c.x, c.y, height)
        features.append({
            "type": "Feature", "geometry": point,
            "properties": {"role": "control", "code": c.code},
        })

    if course.start is not None:
        features.append({
            "type": "Feature", "geometry": _point_geom(course.start[0], course.start[1], height),
            "properties": {"role": "start"},
        })

    if course.finish is not None:
        features.append({
            "type": "Feature", "geometry": _point_geom(course.finish[0], course.finish[1], height),
            "properties": {"role": "finish"},
        })

    for leg in course.legs:
        features.append(_geom_feature(leg, height, {"role": "course_leg"}))

    return {
        "type": "FeatureCollection",
        "features": features,
        # Non-standard top-level members (RFC 7946 permits foreign members on
        # the FeatureCollection object). Consumed by visualize.py and, later,
        # the Flutter renderer to overlay this on the rectified photo without
        # distortion -- not meant for generic third-party GeoJSON tools.
        "properties": {
            "source_photo": source_filename,
            "image_width_px": width,
            "image_height_px": height,
            "scale_to_original_photo": preprocess.scale_to_original,
            "magnetic_north_line_spacing_px": preprocess.mn_line_spacing_px,
            "quad_rectification_found": preprocess.quad_found,
        },
    }


def _point_geom(x: float, y: float, height: int) -> dict:
    return {"type": "Point", "coordinates": [x, height - y]}
