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
from pipeline.segmentation import SegmentationResult

TERRAIN_COLORS_BGR = {
    "forest": (60, 140, 40),
    "clearing": (40, 200, 220),
    "thicket": (20, 80, 20),
    "water": (200, 80, 20),
    "rock": (180, 180, 180),
    "out_of_bounds": (150, 0, 150),
}
PATH_COLOR = (0, 0, 255)
LEG_COLOR = (255, 0, 255)
CONTROL_COLOR = (255, 0, 0)
START_COLOR = (0, 200, 0)
FINISH_COLOR = (0, 220, 220)


def render_qa_overlay(img: np.ndarray, seg: SegmentationResult, course: CourseResult) -> np.ndarray:
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

    return blended
