//! Runs `rectify_photo` against real photographed maps from the Phase 0
//! prototype's test data (see `python_prototype/testData/`, in-scope files
//! per `python_prototype/pipeline/config.IN_SCOPE_FILES`) -- there's no
//! synthetic-image substitute for "does paper-quad detection work on a real
//! photo", so this is an invariant check (dimensions are sane, encoding
//! round-trips), not a pixel-exact one, same reasoning as the Python
//! prototype's `test_pipeline_golden.py`.

use std::path::PathBuf;

use rust_core::api::preprocessing::rectify_photo;

fn test_data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("python_prototype")
        .join("testData")
}

fn check_photo(filename: &str) {
    let path = test_data_dir().join(filename);
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("failed to read {path:?}: {e}"));

    let result = rectify_photo(bytes).unwrap_or_else(|e| panic!("rectify_photo failed on {filename}: {e}"));

    assert!(result.width > 0 && result.height > 0, "{filename}: empty output image");
    assert!(!result.image_png.is_empty(), "{filename}: empty PNG bytes");
    // PNG magic bytes.
    assert_eq!(&result.image_png[0..8], &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A], "{filename}: not a valid PNG");

    println!(
        "{filename}: quad_found={} {}x{}",
        result.quad_found, result.width, result.height
    );
}

#[test]
fn rectify_map0() {
    check_photo("map0.jpg");
}

#[test]
fn rectify_map2() {
    check_photo("map2.jpg");
}

#[test]
fn rectify_map4() {
    check_photo("map4.jpg");
}

#[test]
fn rectify_map6() {
    check_photo("map6.jpg");
}
