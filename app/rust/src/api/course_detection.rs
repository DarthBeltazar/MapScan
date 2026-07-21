//! Rectified map image -> course overprint (controls, start, finish, legs).
//! Conceptual Rust port of the Phase 0 Python prototype's
//! `course_detection.py` -- see its module docstring for why course color is
//! deliberately NOT treated as a fixed hue (red/maroon/magenta all show up
//! depending on the event): this casts a broad warm-hue net for "course ink"
//! and classifies connected components by *shape* (circle/triangle/
//! thin-line), which is what actually stays constant across maps.
//!
//! OCR of printed control codes (`ocr_control_code` in the Python version)
//! is NOT ported here -- it depends on a separately-installed Tesseract
//! binary, which is a distinct infrastructure decision for Android/Flutter
//! (bundling or requiring a native OCR engine) rather than something to pull
//! in incidentally while porting shape detection. `Control::code` is always
//! `None` for now.

use opencv::core::{self, Mat, Point, Scalar, Vector};
use opencv::prelude::*;
use opencv::imgproc;

use super::geometry::{Pt, Segment};
use super::mask_utils::{mask_to_mat, mat_to_mask};
use super::segmentation::hough_segments;

#[derive(Debug, Clone)]
pub struct Control {
    pub x: f32,
    pub y: f32,
    pub radius: f32,
    /// OCR'd control code, e.g. "46" from a "7-46" label. Always `None` for
    /// now -- see module docs.
    pub code: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct CourseResult {
    pub controls: Vec<Control>,
    pub start: Option<Pt>,
    pub finish: Option<Pt>,
    pub legs: Vec<Segment>,
}

/// Broad warm red/maroon/magenta mask -- covers every course-overprint color
/// seen across the Python prototype's test photos. Deliberately wide;
/// shape-based classification downstream is what separates real course
/// marks from other warm-colored map content.
pub(crate) fn build_course_ink_mask(img: &Mat, valid_mask: &[u8]) -> opencv::Result<Vec<u8>> {
    let (width, height) = (img.cols(), img.rows());
    let mut hsv = Mat::default();
    imgproc::cvt_color_def(img, &mut hsv, imgproc::COLOR_BGR2HSV)?;
    let hsv_bytes = hsv.data_bytes()?;

    let mut mask = vec![0u8; (width * height) as usize];
    for i in 0..mask.len() {
        let (h, s, v) = (hsv_bytes[i * 3], hsv_bytes[i * 3 + 1], hsv_bytes[i * 3 + 2]);
        let red_low = h <= 15 || h >= 150;
        let warm_enough = s > 70 && v > 40 && v < 235;
        if red_low && warm_enough && valid_mask[i] != 0 {
            mask[i] = 255;
        }
    }

    let mat = mask_to_mat(&mask, width, height)?;
    let kernel = Mat::new_rows_cols_with_default(3, 3, core::CV_8U, Scalar::all(1.0))?;
    let mut closed = Mat::default();
    imgproc::morphology_ex_def(&mat, &mut closed, imgproc::MORPH_CLOSE, &kernel)?;
    mat_to_mask(&closed)
}

/// Fraction of a candidate circle's own circumference that actually has ink
/// under it, sampled every 2.5 degrees and tolerant of the Hough radius
/// estimate being off by a few px (`band`). Ported as-is from the Python
/// prototype's `_ring_ink_coverage` -- see `RING_COVERAGE_MIN_FRACTION`'s
/// caller-side comment for why this, not a radius-consistency filter, is
/// what actually separates real control rings from Hough false positives.
fn ring_ink_coverage(mask: &[u8], width: i32, height: i32, x: f32, y: f32, r: f32) -> f32 {
    const BAND: i32 = 3;
    const SAMPLES: i32 = 144;
    let mut hits = 0;
    for i in 0..SAMPLES {
        let theta = 2.0 * std::f32::consts::PI * i as f32 / SAMPLES as f32;
        for dr in -BAND..=BAND {
            let px = (x + (r + dr as f32) * theta.cos()) as i32;
            let py = (y + (r + dr as f32) * theta.sin()) as i32;
            if px >= 0 && px < width && py >= 0 && py < height && mask[(py * width + px) as usize] != 0 {
                hits += 1;
                break;
            }
        }
    }
    hits as f32 / SAMPLES as f32
}

/// Control circles: hollow rings of a fairly consistent diameter across the
/// whole map. `param2` is the caller-supplied HoughCircles accumulator
/// threshold -- deliberately not a hardcoded per-file table here (see the
/// Python prototype's `config.HOUGH_PARAM2` and `PHASE0_HANDOFF.md`'s
/// guidance that it doesn't generalize to an unseen photo and belongs behind
/// a manual-correction/sensitivity-tuning UI control instead).
pub(crate) fn detect_controls(
    mask: &[u8],
    width: i32,
    height: i32,
    param2: f64,
    ring_coverage_min_fraction: f32,
) -> opencv::Result<Vec<Control>> {
    let diag = ((width * width + height * height) as f64).sqrt();
    let mut masked = mask.to_vec();
    // The sheet's own printed border is a long, strong, near-circular-at-
    // corners edge that would otherwise compete with real control circles.
    let (mx, my) = ((width as f64 * 0.015) as i32, (height as f64 * 0.015) as i32);
    for y in 0..height {
        for x in 0..width {
            if y < my || y >= height - my || x < mx || x >= width - mx {
                masked[(y * width + x) as usize] = 0;
            }
        }
    }
    let mask_mat = mask_to_mat(&masked, width, height)?;
    let mut blurred = Mat::default();
    imgproc::gaussian_blur_def(&mask_mat, &mut blurred, core::Size::new(5, 5), 0.0)?;

    let mut circles = Vector::<core::Vec3f>::new();
    imgproc::hough_circles(
        &blurred,
        &mut circles,
        imgproc::HOUGH_GRADIENT,
        1.2,
        diag * 0.02,
        60.0,
        param2,
        (diag * 0.006) as i32,
        (diag * 0.02) as i32,
    )?;

    let mut out: Vec<Control> = Vec::new();
    for c in circles.iter() {
        let (x, y, r) = (c[0], c[1], c[2]);
        // HoughCircles returns candidates ordered by accumulator strength
        // (best first); skip anything too close to an already-kept circle
        // instead of double-detecting the same ring.
        if out.iter().any(|k| ((x - k.x).powi(2) + (y - k.y).powi(2)).sqrt() < r) {
            continue;
        }
        if ring_ink_coverage(mask, width, height, x, y, r) < ring_coverage_min_fraction {
            continue;
        }
        out.push(Control { x, y, radius: r, code: None });
    }
    Ok(out)
}

/// Finish is drawn as two concentric circles. Hough reports both as
/// separate detections with close centers and different radii -- merge the
/// closest such pair into a single finish point and drop both from the
/// control list.
fn dedupe_finish(controls: Vec<Control>) -> (Vec<Control>, Option<Pt>) {
    for i in 0..controls.len() {
        for j in (i + 1)..controls.len() {
            let (a, b) = (&controls[i], &controls[j]);
            let dist = ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt();
            if dist < a.radius.max(b.radius) * 0.5 && (a.radius - b.radius).abs() > 1.5 {
                let finish = Pt { x: (a.x + b.x) / 2.0, y: (a.y + b.y) / 2.0 };
                let remaining =
                    controls.iter().enumerate().filter(|&(k, _)| k != i && k != j).map(|(_, c)| c.clone()).collect();
                return (remaining, Some(finish));
            }
        }
    }
    (controls, None)
}

fn blank_disks(mask: &[u8], width: i32, height: i32, disks: &[(f32, f32, f32)]) -> opencv::Result<Mat> {
    let mut mat = mask_to_mat(mask, width, height)?;
    for &(x, y, r) in disks {
        imgproc::circle(&mut mat, Point::new(x as i32, y as i32), r as i32, Scalar::all(0.0), -1, imgproc::LINE_8, 0)?;
    }
    Ok(mat)
}

/// Start is a triangle (not a circle) in the same ink. This reliably returns
/// `None` on the Python prototype's in-scope photos -- see
/// `course_detection.detect_start_triangle`'s docstring in
/// `python_prototype/pipeline/course_detection.py` for the full account of
/// five separate approaches that were tried and failed for concrete,
/// verified reasons (an unguarded shape search finds noise, not a real
/// triangle; tighter HSV recalibration, illumination flattening, and
/// stroke-thickness comparison show no separating signal; rotation-invariant
/// template matching found a false peak on a line-crossing, not a triangle).
/// Reliably finding it would need a trained symbol classifier -- Phase 3 ML
/// territory, not a Phase 0/1 CV heuristic. This port keeps the same
/// size/equilateral-ness gate so the behavior (usually `None`) matches, not
/// because the gate is expected to start working better in Rust.
pub(crate) fn detect_start_triangle(
    mask: &[u8],
    width: i32,
    height: i32,
    controls: &[Control],
    area_ratio_range: (f64, f64),
    min_equilateral_score: f64,
) -> opencv::Result<Option<Pt>> {
    if controls.is_empty() {
        return Ok(None);
    }
    let disks: Vec<(f32, f32, f32)> = controls.iter().map(|c| (c.x, c.y, c.radius * 1.4)).collect();
    let work = blank_disks(mask, width, height, &disks)?;

    let mut contours = Vector::<Vector<Point>>::new();
    imgproc::find_contours_def(&work, &mut contours, imgproc::RETR_EXTERNAL, imgproc::CHAIN_APPROX_SIMPLE)?;

    let mut circle_areas: Vec<f64> = controls.iter().map(|c| std::f64::consts::PI * (c.radius as f64).powi(2)).collect();
    circle_areas.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median_circle_area = circle_areas[circle_areas.len() / 2];
    let min_area = area_ratio_range.0 * median_circle_area;
    let max_area = area_ratio_range.1 * median_circle_area;

    let mut best: Option<Pt> = None;
    let mut best_score = -1.0f64;
    for c in contours.iter() {
        let area = imgproc::contour_area_def(&c)?;
        if area < min_area || area > max_area {
            continue;
        }
        let peri = imgproc::arc_length(&c, true)?;
        let mut approx = Vector::<Point>::new();
        imgproc::approx_poly_dp(&c, &mut approx, 0.04 * peri, true)?;
        if approx.len() != 3 {
            continue;
        }
        let pts: Vec<(f64, f64)> = approx.iter().map(|p| (p.x as f64, p.y as f64)).collect();
        let sides: Vec<f64> = (0..3)
            .map(|i| {
                let (ax, ay) = pts[i];
                let (bx, by) = pts[(i + 1) % 3];
                ((ax - bx).powi(2) + (ay - by).powi(2)).sqrt()
            })
            .collect();
        let mean = sides.iter().sum::<f64>() / 3.0;
        let variance = sides.iter().map(|s| (s - mean).powi(2)).sum::<f64>() / 3.0;
        let std = variance.sqrt();
        let equilateral = 1.0 - (std / mean.max(1e-6));
        if equilateral > best_score {
            best_score = equilateral;
            let cx = pts.iter().map(|p| p.0).sum::<f64>() / 3.0;
            let cy = pts.iter().map(|p| p.1).sum::<f64>() / 3.0;
            best = Some(Pt { x: cx as f32, y: cy as f32 });
        }
    }
    if best_score < min_equilateral_score {
        return Ok(None);
    }
    Ok(best)
}

/// Connecting lines between controls: what's left of the ink mask once
/// circles/triangle are carved out is thin line segments -- detected the
/// same way as `segmentation`'s path lines.
pub(crate) fn detect_legs(
    mask: &[u8],
    width: i32,
    height: i32,
    controls: &[Control],
    start: Option<Pt>,
) -> opencv::Result<Vec<Segment>> {
    let mut disks: Vec<(f32, f32, f32)> = controls.iter().map(|c| (c.x, c.y, c.radius * 1.4)).collect();
    if let Some(s) = start {
        disks.push((s.x, s.y, height as f32 * 0.02));
    }
    let mut work = blank_disks(mask, width, height, &disks)?;

    // Same border-frame exclusion as detect_controls, and for the same
    // reason (the sheet's own printed border otherwise gets traced as a
    // spurious "leg" running the full width/height of the sheet).
    let (mx, my) = ((width as f64 * 0.015) as i32, (height as f64 * 0.015) as i32);
    imgproc::rectangle(
        &mut work,
        core::Rect::new(0, 0, width, my),
        Scalar::all(0.0),
        -1,
        imgproc::LINE_8,
        0,
    )?;
    imgproc::rectangle(
        &mut work,
        core::Rect::new(0, height - my, width, my),
        Scalar::all(0.0),
        -1,
        imgproc::LINE_8,
        0,
    )?;
    imgproc::rectangle(&mut work, core::Rect::new(0, 0, mx, height), Scalar::all(0.0), -1, imgproc::LINE_8, 0)?;
    imgproc::rectangle(
        &mut work,
        core::Rect::new(width - mx, 0, mx, height),
        Scalar::all(0.0),
        -1,
        imgproc::LINE_8,
        0,
    )?;

    let mut edges = Mat::default();
    imgproc::canny_def(&work, &mut edges, 40.0, 120.0)?;
    // minLineLength scaled to image size, well above a control-code digit's
    // stroke length -- see the Python prototype's detect_legs comment on why
    // (short text like "7-46" next to each circle otherwise floods this with
    // thousands of spurious tiny segments).
    let min_len = height as f64 * 0.04;
    hough_segments(&edges, 20, min_len, 15.0)
}

/// Runs control/start/finish/leg detection against an already-computed
/// course-ink mask (`build_course_ink_mask`) -- the caller builds that mask
/// once and reuses it both here and to exclude course ink from
/// `segmentation`'s terrain classification (see the Python prototype's
/// `CourseResult.ink_mask` field, which exists for exactly that reason).
pub(crate) fn detect_course(
    mask: &[u8],
    width: i32,
    height: i32,
    hough_param2: f64,
    ring_coverage_min_fraction: f32,
    start_triangle_area_ratio_range: (f64, f64),
    start_triangle_min_equilateral_score: f64,
) -> opencv::Result<CourseResult> {
    let controls = detect_controls(mask, width, height, hough_param2, ring_coverage_min_fraction)?;
    let (controls, finish) = dedupe_finish(controls);
    let start = detect_start_triangle(
        mask,
        width,
        height,
        &controls,
        start_triangle_area_ratio_range,
        start_triangle_min_equilateral_score,
    )?;
    let legs = detect_legs(mask, width, height, &controls, start)?;

    Ok(CourseResult { controls, start, finish, legs })
}
