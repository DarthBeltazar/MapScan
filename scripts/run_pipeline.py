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
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import cv2

from pipeline.course_detection import detect_course
from pipeline.preprocessing import preprocess_image
from pipeline.segmentation import default_valid_mask, segment_terrain
from pipeline.vectorize import build_feature_collection
from pipeline.visualize import render_qa_overlay


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

    fc = build_feature_collection(seg, course, pre, source_filename)

    os.makedirs(out_dir, exist_ok=True)
    stem = os.path.splitext(source_filename)[0]
    geojson_path = os.path.join(out_dir, f"{stem}.geojson")
    with open(geojson_path, "w", encoding="utf-8") as f:
        json.dump(fc, f, ensure_ascii=False, indent=1)

    qa_img = render_qa_overlay(pre.image, seg, course)
    qa_path = os.path.join(out_dir, f"{stem}_qa.png")
    cv2.imwrite(qa_path, qa_img)

    print(f"[{source_filename}] wrote {geojson_path} ({len(fc['features'])} features) "
          f"and {qa_path} in {time.time() - t0:.1f}s")
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
