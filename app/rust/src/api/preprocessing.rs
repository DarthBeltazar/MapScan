//! Photo -> rectified, color-normalized image. Conceptual Rust port of the
//! Phase 0 Python prototype's `preprocessing.find_paper_quad` /
//! `preprocessing.rectify` / `preprocessing.white_balance` /
//! `preprocessing.detect_magnetic_north_lines` (see
//! `python_prototype/pipeline/preprocessing.py` and `PHASE0_HANDOFF.md`) --
//! not a line-for-line port. Reading-orientation correction is deliberately
//! NOT reimplemented here: the Python prototype's version is a hardcoded
//! per-file rotation table (`config.PAGE_ROTATION_K`) that `PHASE0_HANDOFF.md`
//! explicitly says not to carry into Phase 1 as-is, since it doesn't
//! generalize to a photo this repo hasn't seen -- that knob belongs in the
//! manual-correction UI instead.

use opencv::core::{self, Mat, Point, Point2f, Scalar, Size, Vec4i, Vector};
use opencv::prelude::*;
use opencv::{imgcodecs, imgproc};

const WORKING_MAX_SIDE: i32 = 3000;

#[derive(Debug, Clone)]
pub struct RectifyResult {
    /// Rectified (or, if no confident quad was found, merely downscaled)
    /// image, PNG-encoded.
    pub image_png: Vec<u8>,
    /// False => paper-quad detection fell back to using the full frame.
    pub quad_found: bool,
    pub width: i32,
    pub height: i32,
    /// X positions (px, in the output image's coordinates) of detected
    /// magnetic-north lines. Best-effort -- frequently empty, see
    /// `detect_magnetic_north_lines`.
    pub mn_line_xs: Vec<f32>,
    /// Median spacing (px) between `mn_line_xs`, if at least 2 were found.
    pub mn_line_spacing_px: Option<f32>,
    /// `working_resolution_size / original_photo_size` (<=1.0) -- multiply a
    /// working-resolution pixel coordinate by `1.0 / scale_to_original` to
    /// recover its position in the original, full-resolution photo.
    pub scale_to_original: f32,
}

pub fn rectify_photo(image_bytes: Vec<u8>) -> Result<RectifyResult, String> {
    rectify_photo_impl(image_bytes).map_err(|e| e.to_string())
}

/// Result of the full preprocessing chain, kept as an in-memory `Mat` (not
/// yet PNG-encoded) so downstream stages (`segmentation`, `course_detection`)
/// can run against the same rectified image without a decode round-trip.
/// `#[frb(ignore)]` because this is purely an internal handoff type between
/// Rust modules -- flutter_rust_bridge scans everything under `crate::api`
/// regardless of `pub`/`pub(crate)`, so without this it leaks into the Dart
/// API surface as an opaque class nobody should call.
#[flutter_rust_bridge::frb(ignore)]
pub(crate) struct Preprocessed {
    pub rectified: Mat,
    pub quad_found: bool,
    pub mn_line_xs: Vec<f32>,
    pub mn_line_spacing_px: Option<f32>,
    /// `working_resolution_size / original_photo_size` (<=1.0) -- multiply a
    /// working-resolution pixel coordinate by `1.0 / scale_to_original` to
    /// recover its position in the original, full-resolution photo. Matches
    /// the Python prototype's `PreprocessResult.scale_to_original`.
    pub scale_to_original: f32,
}

pub(crate) fn preprocess(image_bytes: &[u8]) -> opencv::Result<Preprocessed> {
    let buf = Vector::<u8>::from_slice(image_bytes);
    let img = imgcodecs::imdecode(
        &buf,
        imgcodecs::IMREAD_COLOR | imgcodecs::IMREAD_IGNORE_ORIENTATION,
    )?;
    if img.empty() {
        return Err(opencv::Error::new(
            core::StsError,
            "could not decode image bytes".to_string(),
        ));
    }

    let (small, scale_to_original) = downscale(&img, WORKING_MAX_SIDE)?;

    let quad = find_paper_quad(&small)?;
    let (rectified, quad_found) = match quad {
        Some(q) => (rectify(&small, &q)?, true),
        None => (small, false),
    };
    let rectified = white_balance(&rectified, 97.0)?;
    let (mn_line_xs, mn_line_spacing_px) = detect_magnetic_north_lines(&rectified, 10.0)?;

    Ok(Preprocessed {
        rectified,
        quad_found,
        mn_line_xs,
        mn_line_spacing_px,
        scale_to_original: scale_to_original as f32,
    })
}

fn rectify_photo_impl(image_bytes: Vec<u8>) -> opencv::Result<RectifyResult> {
    let pre = preprocess(&image_bytes)?;

    let mut out_buf = Vector::<u8>::new();
    imgcodecs::imencode_def(".png", &pre.rectified, &mut out_buf)?;

    Ok(RectifyResult {
        width: pre.rectified.cols(),
        height: pre.rectified.rows(),
        image_png: out_buf.to_vec(),
        quad_found: pre.quad_found,
        mn_line_xs: pre.mn_line_xs,
        mn_line_spacing_px: pre.mn_line_spacing_px,
        scale_to_original: pre.scale_to_original,
    })
}

/// Returns (downscaled image, scale) where `scale = downscaled_size /
/// original_size` (<=1.0) -- multiply a working-resolution pixel coordinate
/// by `1.0 / scale` to recover its position in the original, full-resolution
/// photo. Matches the Python prototype's `preprocessing.downscale`.
fn downscale(img: &Mat, max_side: i32) -> opencv::Result<(Mat, f64)> {
    let (w, h) = (img.cols(), img.rows());
    let longer = w.max(h);
    if longer <= max_side {
        return Ok((img.clone(), 1.0));
    }
    let scale = max_side as f64 / longer as f64;
    let mut small = Mat::default();
    imgproc::resize(
        img,
        &mut small,
        Size::new(
            (w as f64 * scale).round() as i32,
            (h as f64 * scale).round() as i32,
        ),
        0.0,
        0.0,
        imgproc::INTER_AREA,
    )?;
    Ok((small, scale))
}

/// Order 4 corner points as (top-left, top-right, bottom-right, bottom-left).
fn order_corners(pts: &[Point2f; 4]) -> [Point2f; 4] {
    let (mut tl, mut br, mut tr, mut bl) = (pts[0], pts[0], pts[0], pts[0]);
    let (mut min_s, mut max_s, mut min_d, mut max_d) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
    for p in pts {
        let s = p.x + p.y;
        let d = p.y - p.x;
        if s < min_s {
            min_s = s;
            tl = *p;
        }
        if s > max_s {
            max_s = s;
            br = *p;
        }
        if d < min_d {
            min_d = d;
            tr = *p;
        }
        if d > max_d {
            max_d = d;
            bl = *p;
        }
    }
    [tl, tr, br, bl]
}

/// Largest-contour + convex-hull polygon approximation, with a
/// rotated-bounding-box fallback -- see `preprocessing.find_paper_quad`'s
/// docstring in the Python prototype for why the hull (not the raw contour)
/// is used, and why an undercut approximation falls back to `minAreaRect`
/// rather than being trusted as-is.
fn find_paper_quad(img: &Mat) -> opencv::Result<Option<[Point2f; 4]>> {
    let mut gray = Mat::default();
    imgproc::cvt_color_def(img, &mut gray, imgproc::COLOR_BGR2GRAY)?;
    let mut blurred = Mat::default();
    imgproc::gaussian_blur_def(&gray, &mut blurred, Size::new(5, 5), 0.0)?;
    let mut edges = Mat::default();
    imgproc::canny_def(&blurred, &mut edges, 40.0, 120.0)?;
    let kernel = Mat::new_rows_cols_with_default(5, 5, core::CV_8U, Scalar::all(1.0))?;
    let mut dilated = Mat::default();
    imgproc::dilate(
        &edges,
        &mut dilated,
        &kernel,
        Point::new(-1, -1),
        2,
        core::BORDER_CONSTANT,
        Scalar::all(f64::MAX),
    )?;

    let mut contours = Vector::<Vector<Point>>::new();
    imgproc::find_contours_def(
        &dilated,
        &mut contours,
        imgproc::RETR_EXTERNAL,
        imgproc::CHAIN_APPROX_SIMPLE,
    )?;
    if contours.is_empty() {
        return Ok(None);
    }

    let img_area = (img.rows() as f64) * (img.cols() as f64);
    let mut largest = contours.get(0)?;
    let mut largest_area = imgproc::contour_area_def(&largest)?;
    for c in contours.iter().skip(1) {
        let area = imgproc::contour_area_def(&c)?;
        if area > largest_area {
            largest_area = area;
            largest = c;
        }
    }
    if largest_area < 0.25 * img_area {
        return Ok(None);
    }

    let mut hull = Vector::<Point>::new();
    imgproc::convex_hull_def(&largest, &mut hull)?;
    let hull_area = imgproc::contour_area_def(&hull)?;
    let peri = imgproc::arc_length(&hull, true)?;

    let mut quad: Option<Vector<Point>> = None;
    for eps_frac in [0.01, 0.02, 0.03, 0.05, 0.08] {
        let mut approx = Vector::<Point>::new();
        imgproc::approx_poly_dp(&hull, &mut approx, eps_frac * peri, true)?;
        if approx.len() == 4 {
            quad = Some(approx);
            break;
        }
    }

    if let Some(q) = &quad {
        let quad_area = imgproc::contour_area_def(q)?;
        if quad_area >= 0.85 * hull_area {
            let pts: Vec<Point2f> = q.iter().map(|p| Point2f::new(p.x as f32, p.y as f32)).collect();
            return Ok(Some(order_corners(&[pts[0], pts[1], pts[2], pts[3]])));
        }
    }

    let rect = imgproc::min_area_rect(&hull)?;
    let mut box_mat = Mat::default();
    imgproc::box_points(rect, &mut box_mat)?;
    let box_mat = box_mat.reshape_def(Point2f::opencv_channels())?;
    let pts = [
        *box_mat.at::<Point2f>(0)?,
        *box_mat.at::<Point2f>(1)?,
        *box_mat.at::<Point2f>(2)?,
        *box_mat.at::<Point2f>(3)?,
    ];
    Ok(Some(order_corners(&pts)))
}

fn point_dist(a: Point2f, b: Point2f) -> f32 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
}

/// Perspective-warp `img` so `quad` (tl, tr, br, bl) maps onto an
/// axis-aligned rectangle.
fn rectify(img: &Mat, quad: &[Point2f; 4]) -> opencv::Result<Mat> {
    let [tl, tr, br, bl] = *quad;
    let width_top = point_dist(tr, tl);
    let width_bottom = point_dist(br, bl);
    let height_left = point_dist(bl, tl);
    let height_right = point_dist(br, tr);
    let out_w = width_top.max(width_bottom).round().max(1.0) as i32;
    let out_h = height_left.max(height_right).round().max(1.0) as i32;

    let src = Vector::<Point2f>::from_slice(&[tl, tr, br, bl]);
    let dst = Vector::<Point2f>::from_slice(&[
        Point2f::new(0.0, 0.0),
        Point2f::new((out_w - 1) as f32, 0.0),
        Point2f::new((out_w - 1) as f32, (out_h - 1) as f32),
        Point2f::new(0.0, (out_h - 1) as f32),
    ]);
    let src_mat = Mat::from_slice(src.as_slice())?;
    let dst_mat = Mat::from_slice(dst.as_slice())?;
    let m = imgproc::get_perspective_transform_def(&src_mat, &dst_mat)?;

    let mut warped = Mat::default();
    imgproc::warp_perspective_def(img, &mut warped, &m, Size::new(out_w, out_h))?;
    Ok(warped)
}

/// Normalize color cast using the photographed paper's own white margin as a
/// reference (percentile white-patch balance): find the brightest pixels in
/// the frame (almost certainly the white paper background, not map ink) and
/// scale each channel so their average becomes neutral. Port of
/// `preprocessing.white_balance` in the Python prototype -- see its
/// docstring for why a *fixed* gain correction wouldn't work here (per-photo
/// lighting cast varies too much; this measures and corrects each photo's
/// own cast from its own paper, not a global constant).
fn white_balance(img: &Mat, percentile: f64) -> opencv::Result<Mat> {
    let mut gray = Mat::default();
    imgproc::cvt_color_def(img, &mut gray, imgproc::COLOR_BGR2GRAY)?;
    let gray_bytes = gray.data_bytes()?;

    let mut hist = [0u32; 256];
    for &v in gray_bytes {
        hist[v as usize] += 1;
    }
    let total: u32 = hist.iter().sum();
    let target_count = (total as f64 * percentile / 100.0).ceil() as u32;
    let mut threshold = 255u8;
    let mut cum = 0u32;
    for (i, &c) in hist.iter().enumerate() {
        cum += c;
        if cum >= target_count {
            threshold = i as u8;
            break;
        }
    }

    let img_bytes = img.data_bytes()?;
    let (mut sum_b, mut sum_g, mut sum_r, mut count) = (0u64, 0u64, 0u64, 0u64);
    for (i, &g) in gray_bytes.iter().enumerate() {
        if g >= threshold {
            let px = i * 3;
            sum_b += img_bytes[px] as u64;
            sum_g += img_bytes[px + 1] as u64;
            sum_r += img_bytes[px + 2] as u64;
            count += 1;
        }
    }
    if count < 100 {
        return Ok(img.clone());
    }

    let (mb, mg, mr) = (
        sum_b as f64 / count as f64,
        sum_g as f64 / count as f64,
        sum_r as f64 / count as f64,
    );
    let target = (mb + mg + mr) / 3.0;
    let gains = [target / mb.max(1e-6), target / mg.max(1e-6), target / mr.max(1e-6)];

    let mut out = img.clone();
    let out_bytes = out.data_bytes_mut()?;
    for px in out_bytes.chunks_exact_mut(3) {
        for (c, gain) in gains.iter().enumerate() {
            px[c] = (px[c] as f64 * gain).round().clamp(0.0, 255.0) as u8;
        }
    }
    Ok(out)
}

#[derive(Debug, Clone, Copy)]
struct MnSegment {
    /// Degrees, folded into [0, 180): 0 = horizontal, 90 = vertical.
    angle_deg: f64,
    mid_x: f64,
    mid_y: f64,
}

/// Convert raw HoughLinesP output into (angle, midpoint) segments, dropping
/// anything shorter than `min_len`. No angle restriction here -- the caller
/// filters to near-vertical, and can only assume that's the right family to
/// look for because reading orientation is expected to already be roughly
/// fixed (paper edges axis-aligned) by the time this runs.
fn all_segments(lines: &Vector<Vec4i>, min_len: f64) -> Vec<MnSegment> {
    let mut out = Vec::new();
    for seg in lines.iter() {
        let (x1, y1, x2, y2) = (seg[0] as f64, seg[1] as f64, seg[2] as f64, seg[3] as f64);
        let (dx, dy) = (x2 - x1, y2 - y1);
        let length = (dx * dx + dy * dy).sqrt();
        if length < min_len {
            continue;
        }
        let angle_deg = dy.atan2(dx).to_degrees().rem_euclid(180.0);
        out.push(MnSegment { angle_deg, mid_x: (x1 + x2) / 2.0, mid_y: (y1 + y2) / 2.0 });
    }
    out
}

/// Drop segments whose midpoint sits within `margin_frac` of any image edge
/// -- the rectified sheet's own outer border is otherwise the single
/// longest, straightest edge in the photo and would get mistaken for part of
/// the magnetic-north-line family.
fn drop_border_segments(segments: Vec<MnSegment>, width: i32, height: i32, margin_frac: f64) -> Vec<MnSegment> {
    let (w, h) = (width as f64, height as f64);
    let (mx, my) = (w * margin_frac, h * margin_frac);
    segments
        .into_iter()
        .filter(|s| s.mid_x >= mx && s.mid_x <= w - mx && s.mid_y >= my && s.mid_y <= h - my)
        .collect()
}

fn median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = sorted.len();
    if n == 0 {
        0.0
    } else if n % 2 == 1 {
        sorted[n / 2]
    } else {
        (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
    }
}

/// Best-effort detection of the equally-spaced magnetic-north lines. Port of
/// `preprocessing.detect_magnetic_north_lines` in the Python prototype --
/// see its docstring for why this deliberately only searches near-vertical
/// (rather than any-angle) line families, and why returning nothing is a
/// common, acceptable outcome rather than something to work around: real
/// photos interrupt these thin printed lines constantly (contours, paths,
/// course overprint, text crossing them), and nothing downstream depends on
/// this succeeding -- spacing is only ever used as an optional local-scale
/// hint.
///
/// Returns (sorted x-positions px, median spacing px | None if <2 lines).
fn detect_magnetic_north_lines(img: &Mat, tol_deg: f64) -> opencv::Result<(Vec<f32>, Option<f32>)> {
    let (width, height) = (img.cols(), img.rows());
    let mut gray = Mat::default();
    imgproc::cvt_color_def(img, &mut gray, imgproc::COLOR_BGR2GRAY)?;
    let mut edges = Mat::default();
    imgproc::canny_def(&gray, &mut edges, 30.0, 90.0)?;

    let min_len = height as f64 * 0.06;
    let mut lines = Vector::<Vec4i>::new();
    imgproc::hough_lines_p(
        &edges,
        &mut lines,
        1.0,
        std::f64::consts::PI / 360.0,
        50,
        min_len,
        90.0,
    )?;

    let segs = all_segments(&lines, min_len);
    let segs = drop_border_segments(segs, width, height, 0.03);

    let near_vertical: Vec<MnSegment> = segs.into_iter().filter(|s| (s.angle_deg - 90.0).abs() <= tol_deg).collect();
    if near_vertical.len() < 3 {
        return Ok((vec![], None));
    }

    let mut sorted = near_vertical;
    sorted.sort_by(|a, b| a.mid_x.partial_cmp(&b.mid_x).unwrap());

    let cluster_gap = width as f64 * 0.01;
    let mut clusters: Vec<Vec<MnSegment>> = vec![vec![sorted[0]]];
    for s in sorted.into_iter().skip(1) {
        let last = clusters.last().unwrap().last().unwrap();
        if s.mid_x - last.mid_x <= cluster_gap {
            clusters.last_mut().unwrap().push(s);
        } else {
            clusters.push(vec![s]);
        }
    }

    // A real magnetic-north line gets chopped into many short Hough segments
    // by everything crossing it, but those fragments collectively span most
    // of the map's height. A stray feature that merely happens to have a
    // near-vertical edge contributes only one or two segments over a small
    // y-range -- require both a minimum member count and a minimum vertical
    // span to tell the two apart.
    let img_h = height as f64;
    let mut centers: Vec<f64> = clusters
        .into_iter()
        .filter(|c| {
            if c.len() < 3 {
                return false;
            }
            let (mut min_y, mut max_y) = (f64::MAX, f64::MIN);
            for s in c {
                min_y = min_y.min(s.mid_y);
                max_y = max_y.max(s.mid_y);
            }
            (max_y - min_y) >= 0.25 * img_h
        })
        .map(|c| c.iter().map(|s| s.mid_x).sum::<f64>() / c.len() as f64)
        .collect();
    centers.sort_by(|a, b| a.partial_cmp(b).unwrap());

    if centers.len() < 3 {
        return Ok((centers.iter().map(|&x| x as f32).collect(), None));
    }

    let spacings: Vec<f64> = centers.windows(2).map(|w| w[1] - w[0]).collect();
    let median_spacing = median(&spacings);
    let mut keep = vec![centers[0]];
    for (x, sp) in centers[1..].iter().zip(spacings.iter()) {
        if (sp - median_spacing).abs() / median_spacing.max(1e-6) < 0.4 {
            keep.push(*x);
        }
    }
    let spacing_out = if keep.len() >= 2 {
        let keep_spacings: Vec<f64> = keep.windows(2).map(|w| w[1] - w[0]).collect();
        Some(median(&keep_spacings) as f32)
    } else {
        None
    };
    Ok((keep.into_iter().map(|x| x as f32).collect(), spacing_out))
}
