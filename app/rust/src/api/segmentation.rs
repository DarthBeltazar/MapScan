//! Rectified map image -> draft terrain polygons + path lines. Conceptual
//! Rust port of the Phase 0 Python prototype's `segmentation.py` -- see its
//! module docstring for the scope reminder (classic forest-ISOM photos only,
//! deliberately draft-quality output for a human to clean up in Phase 1's
//! manual-correction UI, not a finished cartographic classification).
//!
//! Unlike the Python version, polygon output here is a plain simplified
//! pixel-contour ring (`geometry::Polygon`), not a shapely `Polygon` with
//! validity repair -- there's no polygon-validity concept to repair in the
//! first place when the output type doesn't enforce simple-ring validity, so
//! the `make_valid`/`GeometryCollection` handling documented in the Python
//! version's `_mask_to_polygons` has no equivalent needed here. This is a
//! deliberate simplification consistent with "draft quality, human cleans up
//! downstream", not an oversight -- if a future stage needs true polygon
//! validity (e.g. the cost-grid rasterizer), that's the point to add a real
//! geometry crate, not here.

use opencv::core::{self, Mat, Point, Scalar, Size, TermCriteria, Vector};
use opencv::prelude::*;
use opencv::imgproc;

use super::geometry::{Polygon, Pt, Segment};
use super::mask_utils::{mask_and, mask_and_not, mask_not, mask_to_mat, mat_to_mask};

const MIN_POLYGON_AREA_PX: f64 = 60.0;
const SIMPLIFY_EPS_PX: f64 = 2.0;
const VEGETATION_CLUSTER_BLUR_KSIZE: i32 = 21;

/// Fractional (x0, y0, x1, y1) box, in [0, 1] relative to the rectified
/// working-resolution image, to exclude from terrain classification (legend
/// tables, titles, sponsor logos). Caller-supplied, not a hardcoded per-file
/// table baked into this crate -- see `PHASE0_HANDOFF.md`'s explicit
/// guidance that the Python prototype's `config.LEGEND_EXCLUDE_BOXES`
/// doesn't generalize to a photo this repo hasn't seen and belongs behind
/// the manual-correction UI instead.
#[derive(Debug, Clone, Copy)]
pub struct ExcludeBox {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

#[derive(Debug, Clone)]
pub struct ClassPolygons {
    pub class_name: String,
    pub polygons: Vec<Polygon>,
}

#[derive(Debug, Clone)]
pub struct SegmentationResult {
    pub polygons_by_class: Vec<ClassPolygons>,
    pub paths: Vec<Segment>,
}

/// True = eligible for terrain classification. `legend_boxes` are excluded
/// outright; `extra_exclude`, if given (course-overprint ink, typically), is
/// also excluded so it doesn't get classified as terrain.
pub(crate) fn build_valid_mask(
    width: i32,
    height: i32,
    legend_boxes: &[ExcludeBox],
    extra_exclude: Option<&[u8]>,
) -> Vec<u8> {
    let mut mask = vec![255u8; (width * height) as usize];
    for b in legend_boxes {
        let (px0, py0, px1, py1) = (
            ((b.x0 * width as f32) as i32).max(0),
            ((b.y0 * height as f32) as i32).max(0),
            ((b.x1 * width as f32) as i32).min(width),
            ((b.y1 * height as f32) as i32).min(height),
        );
        for y in py0..py1 {
            for x in px0..px1 {
                mask[(y * width + x) as usize] = 0;
            }
        }
    }
    if let Some(extra) = extra_exclude {
        mask = mask_and_not(&mask, extra);
    }
    mask
}

/// Binary mask -> simplified pixel-contour polygons. External contours only
/// (no interior holes) -- an intentional Phase-0 simplification, matching
/// the Python prototype's `_mask_to_polygons`.
fn mask_to_polygons(mask: &[u8], width: i32, height: i32) -> opencv::Result<Vec<Polygon>> {
    let mat = mask_to_mat(mask, width, height)?;
    let kernel = Mat::new_rows_cols_with_default(5, 5, core::CV_8U, Scalar::all(1.0))?;
    let mut closed = Mat::default();
    imgproc::morphology_ex_def(&mat, &mut closed, imgproc::MORPH_CLOSE, &kernel)?;

    let mut contours = Vector::<Vector<Point>>::new();
    imgproc::find_contours_def(&closed, &mut contours, imgproc::RETR_EXTERNAL, imgproc::CHAIN_APPROX_SIMPLE)?;

    let mut out = Vec::new();
    for c in contours.iter() {
        if imgproc::contour_area_def(&c)? < MIN_POLYGON_AREA_PX {
            continue;
        }
        let mut approx = Vector::<Point>::new();
        imgproc::approx_poly_dp(&c, &mut approx, SIMPLIFY_EPS_PX, true)?;
        if approx.len() < 3 {
            continue;
        }
        if imgproc::contour_area_def(&approx)? < MIN_POLYGON_AREA_PX {
            continue;
        }
        let points = approx.iter().map(|p| Pt { x: p.x as f32, y: p.y as f32 }).collect();
        out.push(Polygon { points });
    }
    Ok(out)
}

/// Minimum number of enclosed holes for a candidate `out_of_bounds` blob to
/// be accepted -- see `out_of_bounds_polygons`'s doc comment.
const MIN_HATCH_HOLES: usize = 3;

/// Extracts `out_of_bounds` polygons using the real cross-hatch pattern's
/// *shape*, not just `out_of_bounds_mask`'s color -- see that function's doc
/// comment for the full account of why tightening the color threshold was
/// tried on all 4 in-scope photos and made detection worse, not better, and
/// don't re-attempt that without new evidence.
///
/// Real ISOM out-of-bounds cross-hatching is a diagonal lattice: once closed
/// into a blob by the same `MORPH_CLOSE` step `mask_to_polygons` uses, it has
/// *several* small enclosed holes (the diamond-shaped gaps between hatch
/// lines), not zero or one. Checked directly on map2.jpg, at working
/// resolution (not just the raw photo): its two confirmed real hatching
/// zones produced 3 components with 5, 5, and 7 holes respectively (hole
/// areas ~60-150px^2, fairly uniform), while confirmed false positives on
/// isolated printed control-code/table digits had 0-1 holes (a "0"'s single
/// counter at ~1500px^2 -- an order of magnitude bigger than a hatch cell --
/// or none at all for a "9"). Requiring `MIN_HATCH_HOLES` distinguishes them
/// cleanly on this evidence, without needing a tighter (and, per
/// `out_of_bounds_mask`'s doc comment, already-verified-ineffective) color
/// threshold.
///
/// **Verified end-to-end against all 4 in-scope photos, not just map2.jpg**:
/// map0.jpg correctly stays at 0 polygons (no real hatching visible on that
/// file either, checked directly); map2.jpg went from 29 polygons (mostly
/// false positives) to exactly 1, matching its confirmed-real hatching zone;
/// map6.jpg went from 22 to 2, both cropped and confirmed as real hatching,
/// zero false positives left. map4.jpg went from 31 to 5, of which only 2
/// are real hatching -- the other 3 are still false positives, but on a
/// *different* failure mode than the isolated-digit one this filter targets:
/// cropping them showed merged multi-character title/scale text ("H2,5m",
/// "7500") where adjacent glyphs bridge into one blob under the same
/// `MORPH_CLOSE` and collectively accumulate >=3 counter-holes. Traced this
/// to a separate, pre-existing bug, not a flaw in the hole-count logic
/// itself: `tests/analyze.rs`'s calibrated `legend_boxes` for map4.jpg (and
/// likely map6.jpg) were copied from the Python prototype's
/// `config.LEGEND_EXCLUDE_BOXES`, which is calibrated against Python's
/// *rotated* image (`config.PAGE_ROTATION_K` rotates map4.jpg 270 degrees
/// before that config was derived) -- this Rust pipeline deliberately does
/// NOT rotate (see `preprocessing.rs`'s module doc), so that box lands in
/// the wrong place here. Confirmed directly: map4.jpg's title text is
/// plainly visible, unobscured, top-right in the Rust-rectified image
/// rendered from `analyze_map`, not excluded at all. Fixing that
/// box-rotation mismatch is a separate task, not attempted here.
fn out_of_bounds_polygons(mask: &[u8], width: i32, height: i32) -> opencv::Result<Vec<Polygon>> {
    let mat = mask_to_mat(mask, width, height)?;
    let kernel = Mat::new_rows_cols_with_default(5, 5, core::CV_8U, Scalar::all(1.0))?;
    let mut closed = Mat::default();
    imgproc::morphology_ex_def(&mat, &mut closed, imgproc::MORPH_CLOSE, &kernel)?;

    let mut contours = Vector::<Vector<Point>>::new();
    let mut hierarchy = Vector::<core::Vec4i>::new();
    imgproc::find_contours_with_hierarchy_def(
        &closed,
        &mut contours,
        &mut hierarchy,
        imgproc::RETR_CCOMP,
        imgproc::CHAIN_APPROX_SIMPLE,
    )?;

    let n = contours.len();
    let mut hole_counts = vec![0usize; n];
    for i in 0..n {
        let parent = hierarchy.get(i)?[3];
        if parent >= 0 && imgproc::contour_area_def(&contours.get(i)?)? > 5.0 {
            hole_counts[parent as usize] += 1;
        }
    }

    let mut out = Vec::new();
    for i in 0..n {
        if hierarchy.get(i)?[3] != -1 {
            continue; // not a top-level (outer) contour -- a hole itself
        }
        if hole_counts[i] < MIN_HATCH_HOLES {
            continue;
        }
        let c = contours.get(i)?;
        if imgproc::contour_area_def(&c)? < MIN_POLYGON_AREA_PX {
            continue;
        }
        let mut approx = Vector::<Point>::new();
        imgproc::approx_poly_dp(&c, &mut approx, SIMPLIFY_EPS_PX, true)?;
        if approx.len() < 3 {
            continue;
        }
        let points = approx.iter().map(|p| Pt { x: p.x as f32, y: p.y as f32 }).collect();
        out.push(Polygon { points });
    }
    Ok(out)
}

/// ISOM water: a clearly saturated blue fill (verified against the test
/// photos in the Python prototype -- lakes read as strongly saturated blue,
/// unlike pale low-saturation printed overlay lines sharing the same hue).
fn water_mask(hsv: &Mat) -> opencv::Result<Vec<u8>> {
    let mut mask = Mat::default();
    core::in_range(hsv, &Scalar::new(90.0, 60.0, 60.0, 0.0), &Scalar::new(135.0, 255.0, 255.0, 0.0), &mut mask)?;
    mat_to_mask(&mask)
}

/// Purple/violet, area-filled or hatched (course overprint is excluded
/// upstream via `extra_exclude`, not by hue here -- it's a different color
/// family on every in-scope map, see `course_detection`).
///
/// Checked directly against map2.jpg: real out-of-bounds cross-hatching does
/// get found (confirmed by cropping a detected polygon and visually
/// comparing against the raw photo), but is frequently contaminated by
/// printed maroon control-code/control-description-grid digits ("0", "9")
/// that also fall in this hue range. Root cause, confirmed by sampling HSV
/// directly over one such false-positive digit: `build_course_ink_mask`'s
/// warm-hue exclusion (course ink, which should otherwise remove exactly
/// this kind of printed text before segmentation ever runs) only reliably
/// covers hue >=150 with saturation >70, while this mask's range extends
/// down to hue 135 and saturation as low as 40 -- a digit's anti-aliased/
/// lighter edge pixels (median saturation ~59 in the sampled case, plus a
/// hue-135-150 band course-ink doesn't cover at all) leak through the gap
/// between the two masks' thresholds.
///
/// **Tightening this mask's own hue/saturation range was tried and measured
/// worse, not fixed** -- don't re-attempt without new evidence. Sampled real
/// hatching (two confirmed zones on map2.jpg) against the confirmed false
/// positives above: their saturation distributions are nearly identical at
/// every threshold checked (e.g. at sat<70, real hatching had 14-16% of its
/// pixels below that bar, false-positive digits had 15-18% -- no separating
/// gap). Empirically raising the saturation floor from 40 to 70 (matching
/// `build_course_ink_mask`'s own threshold) was verified end-to-end against
/// all 4 in-scope photos: it did not selectively remove the false positives
/// -- it destroyed real signal *more*: map2.jpg's confirmed-real ~1727px^2
/// hatching polygon shrank to a largest remnant of 121px^2, and map0.jpg's
/// out-of-bounds detection (4 polygons, 912px^2 total) dropped to zero
/// entirely. Raising the hue floor from 135 to 150 (excluding the sub-range
/// course-ink doesn't cover) was worse still: zero out-of-bounds polygons on
/// map0.jpg/map4.jpg/map6.jpg, map2.jpg cut to fragments under 200px^2. Same
/// "statistically indistinguishable in HSV" conclusion `detect_start_triangle`
/// already reached for course-ink-vs-triangle confusion, now independently
/// confirmed for this mask too -- a real fix here would need something other
/// than a tighter color threshold (shape/pattern-based, the way `_marsh_mask`
/// discriminates periodicity rather than hue), not attempted yet.
///
/// Real hatching also tends to come back as several small disconnected
/// polygon fragments rather than one clean zone shape, since a diagonal
/// cross-hatch pattern doesn't fully bridge under `mask_to_polygons`'s small
/// 5x5 `MORPH_CLOSE` kernel -- a separate issue from the false-positive one
/// above, not fixed by adjusting either.
fn out_of_bounds_mask(hsv: &Mat) -> opencv::Result<Vec<u8>> {
    let mut mask = Mat::default();
    core::in_range(hsv, &Scalar::new(135.0, 40.0, 40.0, 0.0), &Scalar::new(170.0, 255.0, 255.0, 0.0), &mut mask)?;
    mat_to_mask(&mask)
}

/// Rock/boulder ISOM symbols are black dot/hatch texture on a light
/// background, not a fill color -- classify by local edge density instead of
/// hue.
fn rock_mask(gray: &Mat, light_mask: &[u8], width: i32, height: i32) -> opencv::Result<Vec<u8>> {
    let mut edges = Mat::default();
    imgproc::canny_def(gray, &mut edges, 60.0, 150.0)?;
    let mut edges_f = Mat::default();
    edges.convert_to(&mut edges_f, core::CV_32F, 1.0 / 255.0, 0.0)?;
    let mut density = Mat::default();
    imgproc::box_filter_def(&edges_f, &mut density, -1, Size::new(15, 15))?;
    let density_vals = density.data_typed::<f32>()?;

    let mut out = vec![0u8; (width * height) as usize];
    for i in 0..out.len() {
        if density_vals[i] > 0.18 && light_mask[i] != 0 {
            out[i] = 255;
        }
    }
    Ok(out)
}

fn scale_inplace(mat: &Mat, factor: f64) -> opencv::Result<Mat> {
    let mut out = Mat::default();
    mat.convert_to(&mut out, -1, factor, 0.0)?;
    Ok(out)
}

/// ISOM marsh: a periodic *horizontal* dash/line screen printed over another
/// area color. Classifies by periodicity (how many times a tall, thin
/// vertical window crosses a strong gray-value transition), not hue -- see
/// the Python prototype's `_marsh_mask` docstring for why an
/// edge-density-after-morphological-close approach (mirroring `rock_mask`)
/// was tried first and rejected: it bridges nearby edges into blobs
/// regardless of whether they're actually periodic, lighting up nearly every
/// contour tangle and road edge, not just marsh.
fn marsh_mask(gray: &Mat, valid_mask: &[u8], exclude: &[u8], width: i32, height: i32) -> opencv::Result<Vec<u8>> {
    let mut sob_y = Mat::default();
    imgproc::sobel_def(gray, &mut sob_y, core::CV_32F, 0, 1)?;
    let mut sob_x = Mat::default();
    imgproc::sobel_def(gray, &mut sob_x, core::CV_32F, 1, 0)?;

    let sob_y_vals = sob_y.data_typed::<f32>()?;
    let sob_x_vals = sob_x.data_typed::<f32>()?;
    let trans_y: Vec<f32> = sob_y_vals.iter().map(|&v| if v.abs() > 25.0 { 1.0 } else { 0.0 }).collect();
    let trans_x: Vec<f32> = sob_x_vals.iter().map(|&v| if v.abs() > 25.0 { 1.0 } else { 0.0 }).collect();

    let trans_y_mat = Mat::new_rows_cols_with_data(height, width, &trans_y)?.try_clone()?;
    let trans_x_mat = Mat::new_rows_cols_with_data(height, width, &trans_x)?.try_clone()?;

    // cv Size is (width, height): a tall thin (1, 25) window counts
    // transitions down a column (sensitive to horizontal stripes), scaled
    // from a mean back to a count, then smoothed across 9 neighboring
    // columns (9, 1) so a real stripe's count is sustained, not a fluke.
    let mut step = Mat::default();
    imgproc::box_filter_def(&trans_y_mat, &mut step, -1, Size::new(1, 25))?;
    let step = scale_inplace(&step, 25.0)?;
    let mut vcount = Mat::default();
    imgproc::box_filter_def(&step, &mut vcount, -1, Size::new(9, 1))?;

    let mut step = Mat::default();
    imgproc::box_filter_def(&trans_x_mat, &mut step, -1, Size::new(25, 1))?;
    let step = scale_inplace(&step, 25.0)?;
    let mut hcount = Mat::default();
    imgproc::box_filter_def(&step, &mut hcount, -1, Size::new(1, 9))?;

    let vcount_vals = vcount.data_typed::<f32>()?;
    let hcount_vals = hcount.data_typed::<f32>()?;
    let periodic: Vec<u8> = vcount_vals
        .iter()
        .zip(hcount_vals.iter())
        .map(|(&vc, &hc)| if vc > 15.0 && vc > hc * 2.0 { 255 } else { 0 })
        .collect();

    let periodic_mat = mask_to_mat(&periodic, width, height)?;
    let kernel = Mat::new_rows_cols_with_default(5, 5, core::CV_8U, Scalar::all(1.0))?;
    let mut opened = Mat::default();
    imgproc::morphology_ex_def(&periodic_mat, &mut opened, imgproc::MORPH_OPEN, &kernel)?;
    let opened_mask = mat_to_mask(&opened)?;

    Ok(mask_and(&mask_and(&opened_mask, valid_mask), &mask_not(exclude)))
}

/// Short line segments for dark, thin linear features (solid/dashed black
/// ISOM path symbols). Deliberately not merged into long polylines -- draft
/// quality, see module docstring.
fn path_lines(gray: &Mat, exclude: &[u8], width: i32, height: i32) -> opencv::Result<Vec<Segment>> {
    let gray_vals = gray.data_bytes()?;
    let dark: Vec<u8> = gray_vals
        .iter()
        .zip(exclude)
        .map(|(&g, &ex)| if g < 90 && ex == 0 { 255 } else { 0 })
        .collect();
    let dark_mat = mask_to_mat(&dark, width, height)?;
    let mut edges = Mat::default();
    imgproc::canny_def(&dark_mat, &mut edges, 40.0, 120.0)?;

    let min_len = (15.0_f64).max(height as f64 * 0.015);
    hough_segments(&edges, 25, min_len, 8.0)
}

pub(crate) fn hough_segments(edges: &Mat, threshold: i32, min_len: f64, max_gap: f64) -> opencv::Result<Vec<Segment>> {
    let mut lines = Vector::<core::Vec4i>::new();
    imgproc::hough_lines_p(edges, &mut lines, 1.0, std::f64::consts::PI / 180.0, threshold, min_len, max_gap)?;
    let mut out = Vec::new();
    for seg in lines.iter() {
        let (x1, y1, x2, y2) = (seg[0] as f32, seg[1] as f32, seg[2] as f32, seg[3] as f32);
        let len = ((x2 - x1).powi(2) + (y2 - y1).powi(2)).sqrt();
        if (len as f64) < min_len {
            continue;
        }
        out.push(Segment { a: Pt { x: x1, y: y1 }, b: Pt { x: x2, y: y2 } });
    }
    Ok(out)
}

/// Map a k-means HSV cluster center to forest/clearing/thicket, per ISOM
/// convention (white/pale = fast forest, yellow/olive = open land,
/// darker/denser green = thicket). Best-effort heuristic, not a fitted
/// classifier -- ported as-is from the Python prototype's
/// `_classify_vegetation_cluster`.
fn classify_vegetation_cluster(h: f32, s: f32, v: f32) -> &'static str {
    if (12.0..=34.0).contains(&h) && v >= 120.0 {
        return "clearing";
    }
    if h > 34.0 && h <= 95.0 {
        return if v < 150.0 || s > 140.0 { "thicket" } else { "forest" };
    }
    "forest"
}

pub(crate) fn segment_terrain(img: &Mat, valid_mask: &[u8], k_clusters: i32) -> opencv::Result<SegmentationResult> {
    let (width, height) = (img.cols(), img.rows());
    let mut hsv = Mat::default();
    imgproc::cvt_color_def(img, &mut hsv, imgproc::COLOR_BGR2HSV)?;
    let mut gray = Mat::default();
    imgproc::cvt_color_def(img, &mut gray, imgproc::COLOR_BGR2GRAY)?;

    let water = mask_and(&water_mask(&hsv)?, valid_mask);
    let out_of_bounds = mask_and(&out_of_bounds_mask(&hsv)?, valid_mask);

    let hsv_bytes = hsv.data_bytes()?;
    let mut light_mask = vec![0u8; (width * height) as usize];
    for i in 0..light_mask.len() {
        let (s, v) = (hsv_bytes[i * 3 + 1], hsv_bytes[i * 3 + 2]);
        if s < 60 && v > 150 && valid_mask[i] != 0 && water[i] == 0 && out_of_bounds[i] == 0 {
            light_mask[i] = 255;
        }
    }
    let rock = rock_mask(&gray, &light_mask, width, height)?;

    let mut remaining = mask_and(&mask_and_not(valid_mask, &water), &mask_not(&out_of_bounds));
    remaining = mask_and_not(&remaining, &rock);
    let marsh = marsh_mask(&gray, valid_mask, &mask_not(&remaining), width, height)?;
    // Marsh's own dash/line ink is dense and dark enough to otherwise get
    // picked up wholesale by path_lines (see Python prototype's
    // segment_terrain comment) -- carve it out before path detection the
    // same way water/out_of_bounds/rock already are.
    remaining = mask_and_not(&remaining, &marsh);

    let paths = path_lines(&gray, &mask_not(&remaining), width, height)?;
    // Path ink itself shouldn't be classified as vegetation fill either.
    let gray_bytes = gray.data_bytes()?;
    for i in 0..remaining.len() {
        if gray_bytes[i] < 90 {
            remaining[i] = 0;
        }
    }

    let mut polygons_by_class = vec![
        ClassPolygons { class_name: "water".to_string(), polygons: mask_to_polygons(&water, width, height)? },
        ClassPolygons {
            class_name: "out_of_bounds".to_string(),
            polygons: out_of_bounds_polygons(&out_of_bounds, width, height)?,
        },
        ClassPolygons { class_name: "rock".to_string(), polygons: mask_to_polygons(&rock, width, height)? },
        ClassPolygons { class_name: "marsh".to_string(), polygons: mask_to_polygons(&marsh, width, height)? },
    ];

    let sample_indices: Vec<usize> = (0..remaining.len()).filter(|&i| remaining[i] != 0).collect();
    let mut veg_masks: [Vec<u8>; 3] = [
        vec![0u8; (width * height) as usize],
        vec![0u8; (width * height) as usize],
        vec![0u8; (width * height) as usize],
    ]; // forest, clearing, thicket

    if sample_indices.len() >= (k_clusters as usize) * 20 {
        // Cluster on a blurred copy, not raw per-pixel HSV -- see the Python
        // prototype's `VEGETATION_CLUSTER_BLUR_KSIZE` comment: unblurred,
        // JPEG noise and fine linework make cluster membership flicker
        // pixel-to-pixel, so mask_to_polygons sees mostly single-pixel
        // specks and drops nearly all of them as noise.
        let mut blurred = Mat::default();
        imgproc::median_blur(img, &mut blurred, VEGETATION_CLUSTER_BLUR_KSIZE)?;
        let mut blurred_hsv = Mat::default();
        imgproc::cvt_color_def(&blurred, &mut blurred_hsv, imgproc::COLOR_BGR2HSV)?;
        let blurred_hsv_bytes = blurred_hsv.data_bytes()?;

        let n = sample_indices.len();
        let mut samples = Vec::with_capacity(n * 3);
        for &i in &sample_indices {
            samples.push(blurred_hsv_bytes[i * 3] as f32);
            samples.push(blurred_hsv_bytes[i * 3 + 1] as f32);
            samples.push(blurred_hsv_bytes[i * 3 + 2] as f32);
        }
        let samples_mat = Mat::new_rows_cols_with_data(n as i32, 3, &samples)?;

        let criteria = TermCriteria {
            typ: core::TermCriteria_EPS + core::TermCriteria_MAX_ITER,
            max_count: 20,
            epsilon: 1.0,
        };
        let mut labels = Mat::default();
        let mut centers = Mat::default();
        core::kmeans(&samples_mat, k_clusters, &mut labels, criteria, 3, core::KMEANS_PP_CENTERS, &mut centers)?;

        let labels_vals = labels.data_typed::<i32>()?;
        let centers_vals = centers.data_typed::<f32>()?;
        for ci in 0..k_clusters as usize {
            let (h_c, s_c, v_c) = (centers_vals[ci * 3], centers_vals[ci * 3 + 1], centers_vals[ci * 3 + 2]);
            let cls = classify_vegetation_cluster(h_c, s_c, v_c);
            let slot = match cls {
                "forest" => 0,
                "clearing" => 1,
                _ => 2,
            };
            for (sample_idx, &label) in labels_vals.iter().enumerate() {
                if label as usize == ci {
                    veg_masks[slot][sample_indices[sample_idx]] = 255;
                }
            }
        }
    }

    for (name, mask) in [("forest", &veg_masks[0]), ("clearing", &veg_masks[1]), ("thicket", &veg_masks[2])] {
        polygons_by_class
            .push(ClassPolygons { class_name: name.to_string(), polygons: mask_to_polygons(mask, width, height)? });
    }

    Ok(SegmentationResult { polygons_by_class, paths })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mask_with_holes(width: i32, height: i32, hole_positions: &[(i32, i32)], hole_size: i32) -> Vec<u8> {
        let mut mask = vec![255u8; (width * height) as usize];
        for &(hx, hy) in hole_positions {
            for dy in 0..hole_size {
                for dx in 0..hole_size {
                    let (x, y) = (hx + dx, hy + dy);
                    if x >= 0 && x < width && y >= 0 && y < height {
                        mask[(y * width + x) as usize] = 0;
                    }
                }
            }
        }
        mask
    }

    #[test]
    fn out_of_bounds_polygons_keeps_a_lattice_with_several_holes() {
        // Several small evenly-spaced holes, standing in for the diamond
        // gaps in a real ISOM cross-hatch pattern -- see this module's
        // `out_of_bounds_polygons` doc comment for why hole *count*, not
        // color, is what discriminates real hatching from false positives
        // (checked directly on map2.jpg/map4.jpg/map6.jpg's real photos).
        let (w, h) = (60, 60);
        let holes = [(8, 8), (24, 8), (40, 8), (8, 24), (24, 24), (40, 24)];
        let mask = mask_with_holes(w, h, &holes, 10);
        let polys = out_of_bounds_polygons(&mask, w, h).unwrap();
        assert_eq!(polys.len(), 1, "a blob with several small holes should be kept as one out_of_bounds polygon");
    }

    #[test]
    fn out_of_bounds_polygons_rejects_a_single_ring_shape() {
        // One big hole, standing in for a printed digit like "0" -- checked
        // directly on map2.jpg that these are the real false-positive shape
        // this filter needs to reject.
        let (w, h) = (60, 60);
        let mask = mask_with_holes(w, h, &[(20, 20)], 20);
        let polys = out_of_bounds_polygons(&mask, w, h).unwrap();
        assert!(polys.is_empty(), "a single-hole ring shape (e.g. a printed digit) should be rejected");
    }

    #[test]
    fn out_of_bounds_polygons_rejects_a_solid_blob_with_no_holes() {
        let (w, h) = (60, 60);
        let mask = mask_with_holes(w, h, &[], 0);
        let polys = out_of_bounds_polygons(&mask, w, h).unwrap();
        assert!(polys.is_empty(), "a solid blob with no holes at all should be rejected");
    }
}
