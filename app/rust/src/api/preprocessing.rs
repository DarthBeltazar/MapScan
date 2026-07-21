//! Photo -> rectified image. Conceptual Rust port of the Phase 0 Python
//! prototype's `preprocessing.find_paper_quad` / `preprocessing.rectify`
//! (see `python_prototype/pipeline/preprocessing.py` and
//! `PHASE0_HANDOFF.md`) -- not a line-for-line port, and deliberately scoped
//! to just perspective correction for this first Phase 1 slice. EXIF-safe
//! loading, white balance, reading-orientation correction and
//! magnetic-north-line detection are not reimplemented yet.

use opencv::core::{self, Mat, Point, Point2f, Scalar, Size, Vector};
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
}

pub fn rectify_photo(image_bytes: Vec<u8>) -> Result<RectifyResult, String> {
    rectify_photo_impl(image_bytes).map_err(|e| e.to_string())
}

fn rectify_photo_impl(image_bytes: Vec<u8>) -> opencv::Result<RectifyResult> {
    let buf = Vector::<u8>::from_slice(&image_bytes);
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

    let small = downscale(&img, WORKING_MAX_SIDE)?;

    let quad = find_paper_quad(&small)?;
    let (rectified, quad_found) = match quad {
        Some(q) => (rectify(&small, &q)?, true),
        None => (small, false),
    };

    let mut out_buf = Vector::<u8>::new();
    imgcodecs::imencode_def(".png", &rectified, &mut out_buf)?;

    Ok(RectifyResult {
        width: rectified.cols(),
        height: rectified.rows(),
        image_png: out_buf.to_vec(),
        quad_found,
    })
}

fn downscale(img: &Mat, max_side: i32) -> opencv::Result<Mat> {
    let (w, h) = (img.cols(), img.rows());
    let longer = w.max(h);
    if longer <= max_side {
        return Ok(img.clone());
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
    Ok(small)
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
