"""CLI: photo -> output/<name>.geojson + output/<name>_qa.png.

    python scripts/run_pipeline.py testData/map0.jpg
    python scripts/run_pipeline.py testData/map0.jpg --no-ocr --out-dir output

Phase-0 prototype scope: classic forest-ISOM map photos only (see plan) --
config.IN_SCOPE_FILES lists which testData files this was tuned against.
Running it on other photos won't raise, but quality is unverified there.
"""

from __future__ import annotations

import argparse
import json
import math
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import cv2

from pipeline.cost_grid import build_cost_grid, route_terrain_breakdown
from pipeline.course_detection import CourseResult, detect_course
from pipeline.pathfinding import RouteResult, find_route
from pipeline.preprocessing import preprocess_image
from pipeline.segmentation import default_valid_mask, segment_terrain
from pipeline.vectorize import build_feature_collection
from pipeline.visualize import render_cost_grid_overlay, render_qa_overlay


def _pick_demo_route_endpoints(
    course: CourseResult,
) -> tuple[tuple[float, float], tuple[float, float]] | None:
    """Phase-0 has no control-sequencing/course-graph yet (course_detection
    only returns an unordered control list) -- so the demo route this picks
    is just "start -> nearest control", or the first two detected controls if
    no start was found. It's a connectivity proof for the cost grid, not a
    real full-course route (see pipeline/pathfinding.py's docstring)."""
    if course.start is not None and course.controls:
        nearest = min(
            course.controls,
            key=lambda c: math.hypot(c.x - course.start[0], c.y - course.start[1]),
        )
        return course.start, (nearest.x, nearest.y)
    if len(course.controls) >= 2:
        a, b = course.controls[0], course.controls[1]
        return (a.x, a.y), (b.x, b.y)
    return None


def run(image_path: str, out_dir: str, run_ocr: bool = True) -> dict:
    source_filename = os.path.basename(image_path)
    t0 = time.time()

    pre = preprocess_image(image_path)
    print(f"[{source_filename}] preprocessed: shape={pre.image.shape}, "
          f"quad_found={pre.quad_found}, mn_lines={len(pre.mn_line_xs)}")

    valid_mask = default_valid_mask(pre.image, source_filename)

    course = detect_course(pre.image, valid_mask, run_ocr=run_ocr, source_filename=source_filename)
    print(f"[{source_filename}] course: {len(course.controls)} controls, "
          f"start={'found' if course.start else 'none'}, "
          f"finish={'found' if course.finish else 'none'}, {len(course.legs)} legs")

    # Course ink shouldn't be classified as terrain fill underneath it.
    seg_mask = valid_mask & ~course.ink_mask
    seg = segment_terrain(pre.image, seg_mask)
    n_polys = sum(len(v) for v in seg.polygons.values())
    print(f"[{source_filename}] segmentation: {n_polys} terrain polygons, {len(seg.paths)} path segments")

    cost_grid = build_cost_grid(seg, pre.image.shape, valid_mask=valid_mask)
    route: RouteResult | None = None
    terrain_breakdown: dict[str, float] | None = None
    endpoints = _pick_demo_route_endpoints(course)
    if endpoints is not None:
        route = find_route(cost_grid.cost, endpoints[0], endpoints[1])
        terrain_breakdown = route_terrain_breakdown(cost_grid.class_grid, route.points)
        # recomputed_cost is a from-scratch resum of this same route against
        # this same grid (pathfinding.RouteResult docstring) -- printing the
        # comparison surfaces a real bug immediately instead of silently
        # trusting route_through_array's own number.
        cost_check = "ok" if math.isclose(route.cost, route.recomputed_cost, rel_tol=1e-3) else "MISMATCH"
        breakdown_str = ", ".join(
            f"{name}={frac * 100:.0f}%" for name, frac in sorted(terrain_breakdown.items(), key=lambda kv: -kv[1])
        )
        print(f"[{source_filename}] demo route: {len(route.points)} points, cost={route.cost:.1f} "
              f"(cross-check {cost_check}), terrain: {breakdown_str}")
    else:
        print(f"[{source_filename}] demo route: skipped (need a start + control, or 2+ controls)")

    fc = build_feature_collection(
        seg, course, pre, source_filename, route=route, route_terrain_breakdown=terrain_breakdown,
    )

    os.makedirs(out_dir, exist_ok=True)
    stem = os.path.splitext(source_filename)[0]
    geojson_path = os.path.join(out_dir, f"{stem}.geojson")
    with open(geojson_path, "w", encoding="utf-8") as f:
        json.dump(fc, f, ensure_ascii=False, indent=1)

    qa_img = render_qa_overlay(pre.image, seg, course, route=route)
    qa_path = os.path.join(out_dir, f"{stem}_qa.png")
    cv2.imwrite(qa_path, qa_img)

    cost_qa_img = render_cost_grid_overlay(cost_grid.cost, route=route)
    cost_qa_path = os.path.join(out_dir, f"{stem}_cost_qa.png")
    cv2.imwrite(cost_qa_path, cost_qa_img)

    print(f"[{source_filename}] wrote {geojson_path} ({len(fc['features'])} features), "
          f"{qa_path}, and {cost_qa_path} in {time.time() - t0:.1f}s")
    return fc


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("image_path", help="Path to a map photo (e.g. testData/map0.jpg)")
    parser.add_argument("--out-dir", default="output", help="Output directory (default: output)")
    parser.add_argument("--no-ocr", action="store_true", help="Skip control-code OCR (faster, or if Tesseract isn't installed)")
    args = parser.parse_args()
    run(args.image_path, args.out_dir, run_ocr=not args.no_ocr)


if __name__ == "__main__":
    main()
