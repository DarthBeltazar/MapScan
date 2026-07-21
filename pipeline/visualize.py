"""Render a QA overlay (segmentation + course detections) on top of the
rectified photo. With no labeled ground truth available for these photos
(see plan), this PNG is the primary way to actually judge output quality --
numeric invariants in the test suite catch regressions, but "is this right"
is a visual question for now.
"""

from __future__ import annotations

import cv2
import numpy as np

from pipeline.course_detection import CourseResult
from pipeline.pathfinding import RouteResult
from pipeline.segmentation import SegmentationResult

# Deliberately *not* colors that echo ISOM's own printed palette (real forest
# is white/pale, clearings are yellow, water is blue) -- an earlier version
# used near-ISOM greens/yellows/blues here and the fill blended into the
# photo's own ink almost invisibly at this alpha, defeating the QA PNG's one
# job (see this module's docstring: it's the primary way to judge output
# quality since there's no ground truth). These are saturated, mutually
# distinct colors absent from real ISOM ink, chosen to visibly stand out
# against forest/clearing/water/rock alike regardless of what's underneath.
TERRAIN_COLORS_BGR = {
    "forest": (0, 255, 0),        # pure green
    "clearing": (255, 0, 255),    # magenta
    "thicket": (0, 128, 255),     # orange
    "water": (0, 255, 255),       # yellow
    "rock": (255, 255, 0),        # cyan
    "out_of_bounds": (255, 0, 0),  # pure blue
}
PATH_COLOR = (0, 0, 255)
LEG_COLOR = (255, 0, 255)
CONTROL_COLOR = (255, 0, 0)
START_COLOR = (0, 200, 0)
FINISH_COLOR = (0, 220, 220)
ROUTE_COLOR = (0, 255, 255)


def render_qa_overlay(
    img: np.ndarray, seg: SegmentationResult, course: CourseResult, route: RouteResult | None = None,
) -> np.ndarray:
    fill_layer = img.copy()
    for cls, polys in seg.polygons.items():
        color = TERRAIN_COLORS_BGR.get(cls, (255, 255, 255))
        for poly in polys:
            pts = np.array(poly.exterior.coords, dtype=np.int32)
            cv2.fillPoly(fill_layer, [pts], color)
    blended = cv2.addWeighted(fill_layer, 0.4, img, 0.6, 0)

    for line in seg.paths:
        (x1, y1), (x2, y2) = list(line.coords)
        cv2.line(blended, (int(x1), int(y1)), (int(x2), int(y2)), PATH_COLOR, 1)

    for leg in course.legs:
        (x1, y1), (x2, y2) = list(leg.coords)
        cv2.line(blended, (int(x1), int(y1)), (int(x2), int(y2)), LEG_COLOR, 2)

    for c in course.controls:
        cv2.circle(blended, (int(c.x), int(c.y)), int(c.radius), CONTROL_COLOR, 3)
        label = c.code or "?"
        cv2.putText(blended, label, (int(c.x + c.radius), int(c.y)),
                    cv2.FONT_HERSHEY_SIMPLEX, 0.6, CONTROL_COLOR, 2, cv2.LINE_AA)

    if course.start is not None:
        cv2.drawMarker(blended, (int(course.start[0]), int(course.start[1])),
                        START_COLOR, cv2.MARKER_TRIANGLE_UP, 36, 4)
    if course.finish is not None:
        cv2.drawMarker(blended, (int(course.finish[0]), int(course.finish[1])),
                        FINISH_COLOR, cv2.MARKER_DIAMOND, 36, 4)

    if route is not None and len(route.points) >= 2:
        pts = np.array(route.points, dtype=np.int32).reshape(-1, 1, 2)
        cv2.polylines(blended, [pts], isClosed=False, color=ROUTE_COLOR, thickness=3)

    return blended
