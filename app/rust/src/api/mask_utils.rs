//! Internal helpers shared by `segmentation` and `course_detection`: boolean
//! masks are represented as flat `Vec<u8>` (0 or 255, row-major, matching
//! OpenCV's own mask convention) rather than a `Mat` everywhere, since most
//! of the per-pixel classification logic ported from the Python prototype
//! (hue thresholds, density comparisons) is easier to express as plain Rust
//! loops than as chained OpenCV ops -- only shape-dependent steps (contours,
//! Hough, morphology) need to go through an actual `Mat`.

use opencv::core::Mat;
use opencv::prelude::*;

pub(crate) fn mask_and(a: &[u8], b: &[u8]) -> Vec<u8> {
    a.iter().zip(b).map(|(&x, &y)| if x != 0 && y != 0 { 255 } else { 0 }).collect()
}

pub(crate) fn mask_and_not(a: &[u8], b: &[u8]) -> Vec<u8> {
    a.iter().zip(b).map(|(&x, &y)| if x != 0 && y == 0 { 255 } else { 0 }).collect()
}

pub(crate) fn mask_not(a: &[u8]) -> Vec<u8> {
    a.iter().map(|&x| if x == 0 { 255 } else { 0 }).collect()
}

/// Copy a flat 0/255 mask into an owned single-channel `CV_8U` `Mat`.
pub(crate) fn mask_to_mat(mask: &[u8], width: i32, height: i32) -> opencv::Result<Mat> {
    Mat::new_rows_cols_with_data(height, width, mask)?.try_clone()
}

/// Read back a single-channel `CV_8U` `Mat` (must be continuous) as a flat
/// 0/255 mask -- "> 0" per OpenCV's own truthiness convention for mask Mats.
pub(crate) fn mat_to_mask(mat: &Mat) -> opencv::Result<Vec<u8>> {
    Ok(mat.data_bytes()?.iter().map(|&v| if v != 0 { 255 } else { 0 }).collect())
}
