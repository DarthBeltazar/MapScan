"""Rectified map image -> draft terrain polygons + path lines.

Scope reminder (see plan): this targets classic forest-ISOM photos only, and
is explicitly draft-quality -- meant for a human to clean up in Phase 1's
manual-correction UI, not a finished cartographic classification. Area
classes (forest/clearing/thicket/water/rock/out_of_bounds) come out as
shapely polygons; paths come out as short line segments (ISOM paths are
linear features, not area fills, so they don't belong in the same
k-means-over-area-color pass as the rest).
"""

from __future__ import annotations

from dataclasses import dataclass, field

import cv2
import numpy as np
from shapely.geometry import LineString, Polygon
from shapely.validation import make_valid

from pipeline.config import LEGEND_EXCLUDE_BOXES

MIN_POLYGON_AREA_PX = 60.0
SIMPLIFY_TOLERANCE_PX = 2.0


@dataclass
class SegmentationResult:
    polygons: dict[str, list[Polygon]] = field(default_factory=dict)  # terrain class -> polygons
    paths: list[LineString] = field(default_factory=list)


def build_valid_mask(
    img_shape: tuple[int, int], legend_boxes: list[tuple[float, float, float, float]],
    extra_exclude_mask: np.ndarray | None = None,
) -> np.ndarray:
    """True = eligible for terrain classification.

    `legend_boxes` are fractional (x0, y0, x1, y1) boxes to exclude (legend
    tables, titles, sponsor logos -- see config.LEGEND_EXCLUDE_BOXES).
    `extra_exclude_mask`, if given, is a boolean array (course-overprint ink,
    typically) also excluded so it doesn't get classified as terrain.
    """
    h, w = img_shape[:2]
    mask = np.ones((h, w), dtype=bool)
    for x0, y0, x1, y1 in legend_boxes:
        px0, py0, px1, py1 = int(x0 * w), int(y0 * h), int(x1 * w), int(y1 * h)
        mask[py0:py1, px0:px1] = False
    if extra_exclude_mask is not None:
        mask &= ~extra_exclude_mask
    return mask


def _mask_to_polygons(mask: np.ndarray) -> list[Polygon]:
    """Binary mask -> simplified, valid shapely polygons.

    External contours only (no interior holes) -- an intentional Phase-0
    simplification consistent with "draft quality, human cleans up in the
    manual-correction UI" (see plan).
    """
    mask_u8 = (mask.astype(np.uint8)) * 255
    mask_u8 = cv2.morphologyEx(mask_u8, cv2.MORPH_CLOSE, np.ones((5, 5), np.uint8))
    contours, _ = cv2.findContours(mask_u8, cv2.RETR_EXTERNAL, cv2.CHAIN_APPROX_SIMPLE)
    polys = []
    for c in contours:
        if cv2.contourArea(c) < MIN_POLYGON_AREA_PX:
            continue
        pts = c.reshape(-1, 2)
        if len(pts) < 3:
            continue
        poly = Polygon(pts)
        if not poly.is_valid:
            # Raw pixel-contour rings routinely self-touch (single-pixel-wide
            # spikes, thin channels) and make_valid's fix for those often
            # comes back as a GeometryCollection mixing polygon pieces with
            # degenerate lines/points, not a plain Polygon/MultiPolygon --
            # verified empirically: silently mishandling that geom_type was
            # dropping the largest 1-2 contours (a whole lake, in one case),
            # not just small noise, so every branch below is exercised.
            poly = make_valid(poly)
        poly = poly.simplify(SIMPLIFY_TOLERANCE_PX, preserve_topology=True)
        if poly.is_empty:
            continue
        if poly.geom_type == "Polygon":
            candidates = [poly]
        elif poly.geom_type == "MultiPolygon":
            candidates = list(poly.geoms)
        elif poly.geom_type == "GeometryCollection":
            candidates = [g for g in poly.geoms if g.geom_type == "Polygon"]
            for g in poly.geoms:
                if g.geom_type == "MultiPolygon":
                    candidates.extend(g.geoms)
        else:
            candidates = []
        polys.extend(g for g in candidates if g.area >= MIN_POLYGON_AREA_PX)
    return polys


def _water_mask(hsv: np.ndarray) -> np.ndarray:
    # ISOM water is a clearly saturated blue fill (verified against the test
    # photos -- lakes read as strongly saturated blue, unlike the pale
    # low-saturation printed overlay lines that share the same hue range).
    return cv2.inRange(hsv, (90, 60, 60), (135, 255, 255)) > 0


def _out_of_bounds_mask(hsv: np.ndarray) -> np.ndarray:
    # Purple/violet, area-filled or hatched (course overprint, which is a
    # different color family on every in-scope map -- see plan -- is
    # excluded upstream via extra_exclude_mask, not by hue here).
    return cv2.inRange(hsv, (135, 40, 40), (170, 255, 255)) > 0


def _rock_mask(gray: np.ndarray, light_mask: np.ndarray) -> np.ndarray:
    """Rock/boulder ISOM symbols are black dot/hatch texture on a light
    background, not a fill color -- classify by local edge density instead
    of hue."""
    edges = cv2.Canny(gray, 60, 150)
    density = cv2.boxFilter(edges.astype(np.float32) / 255.0, -1, (15, 15))
    return (density > 0.18) & light_mask


def _path_lines(gray: np.ndarray, exclude: np.ndarray) -> list[LineString]:
    """Short line segments for dark, thin linear features (the common
    solid/dashed black ISOM path symbols). Deliberately not merged into
    long polylines -- draft quality, see module docstring -- a scattering of
    short segments along each trail is enough for a human to clean up.
    """
    dark = ((gray < 90).astype(np.uint8)) * 255
    dark[exclude] = 0
    edges = cv2.Canny(dark, 40, 120)
    # Paths wind, so segments stay shorter than the course-leg case in
    # course_detection.py, but this still needs to be well above stray
    # text-stroke length to avoid thousands of spurious tiny segments.
    min_len = max(15.0, gray.shape[0] * 0.015)
    lines = cv2.HoughLinesP(edges, 1, np.pi / 180, threshold=25, minLineLength=min_len, maxLineGap=8)
    if lines is None:
        return []
    out = []
    for seg in lines:
        x1, y1, x2, y2 = np.asarray(seg).reshape(-1).astype(float)
        if np.hypot(x2 - x1, y2 - y1) < min_len:
            continue
        out.append(LineString([(x1, y1), (x2, y2)]))
    return out


def _classify_vegetation_cluster(h: float, s: float, v: float) -> str:
    """Map a k-means HSV cluster center to forest/clearing/thicket, per
    ISOM convention (white/pale = fast forest, yellow/olive = open land,
    darker/denser green = thicket). Best-effort heuristic, not a fitted
    classifier -- see plan's testing section on what's actually verifiable
    without labeled ground truth.
    """
    if 12 <= h <= 34 and v >= 120:
        return "clearing"
    if 34 < h <= 95:
        return "thicket" if (v < 150 or s > 140) else "forest"
    return "forest"


def segment_terrain(img: np.ndarray, valid_mask: np.ndarray, k_clusters: int = 6) -> SegmentationResult:
    hsv = cv2.cvtColor(img, cv2.COLOR_BGR2HSV)
    gray = cv2.cvtColor(img, cv2.COLOR_BGR2GRAY)

    water = _water_mask(hsv) & valid_mask
    out_of_bounds = _out_of_bounds_mask(hsv) & valid_mask

    light_mask = (hsv[:, :, 1] < 60) & (hsv[:, :, 2] > 150) & valid_mask & ~water & ~out_of_bounds
    rock = _rock_mask(gray, light_mask)

    remaining = valid_mask & ~water & ~out_of_bounds & ~rock
    paths = _path_lines(gray, exclude=~remaining)
    # Path ink itself shouldn't be classified as vegetation fill either.
    dark_path_mask = gray < 90
    remaining &= ~dark_path_mask

    result = SegmentationResult()
    result.polygons["water"] = _mask_to_polygons(water)
    result.polygons["out_of_bounds"] = _mask_to_polygons(out_of_bounds)
    result.polygons["rock"] = _mask_to_polygons(rock)
    result.paths = paths

    ys, xs = np.where(remaining)
    veg_labels = {"forest": [], "clearing": [], "thicket": []}
    if len(xs) >= k_clusters * 20:
        # Cluster on a blurred copy, not raw per-pixel HSV: unblurred, JPEG
        # noise and fine linework (contour hachures, tiny symbols) make
        # cluster membership flicker pixel-to-pixel, so _mask_to_polygons
        # sees mostly single-pixel specks and drops nearly all of them as
        # noise -- verified empirically, this was silently discarding ~80%+
        # of the vegetation area before adding the blur.
        blurred_hsv = cv2.cvtColor(cv2.medianBlur(img, 9), cv2.COLOR_BGR2HSV)
        samples = blurred_hsv[ys, xs].astype(np.float32)
        criteria = (cv2.TERM_CRITERIA_EPS + cv2.TERM_CRITERIA_MAX_ITER, 20, 1.0)
        _compactness, labels, centers = cv2.kmeans(
            samples, k_clusters, None, criteria, attempts=3, flags=cv2.KMEANS_PP_CENTERS
        )
        labels = labels.reshape(-1)
        cluster_mask = np.zeros(img.shape[:2], dtype=np.uint8)
        for ci in range(k_clusters):
            h_c, s_c, v_c = centers[ci]
            cls = _classify_vegetation_cluster(float(h_c), float(s_c), float(v_c))
            member = labels == ci
            cluster_mask[:] = 0
            cluster_mask[ys[member], xs[member]] = 1
            veg_labels[cls].append(cluster_mask.astype(bool).copy())

    for cls, masks in veg_labels.items():
        if not masks:
            result.polygons[cls] = []
            continue
        combined = np.zeros(img.shape[:2], dtype=bool)
        for m in masks:
            combined |= m
        result.polygons[cls] = _mask_to_polygons(combined)

    return result


def default_valid_mask(img: np.ndarray, source_filename: str) -> np.ndarray:
    """Convenience wrapper: valid mask using this file's configured legend
    exclude boxes (see config.LEGEND_EXCLUDE_BOXES), no extra exclusion."""
    boxes = LEGEND_EXCLUDE_BOXES.get(source_filename, [])
    return build_valid_mask(img.shape, boxes)
