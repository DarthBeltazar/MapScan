//! Plain point/segment/polygon types shared by `segmentation` and
//! `course_detection`'s frb-bridged results. Structs rather than tuples so
//! flutter_rust_bridge codegen has an unambiguous Dart-side shape.

#[derive(Debug, Clone, Copy)]
pub struct Pt {
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct Segment {
    pub a: Pt,
    pub b: Pt,
}

/// Simplified polygon ring, exterior only (no holes) -- draft-quality, see
/// `segmentation` module docs for why this doesn't attempt shapely-style
/// polygon validity repair.
#[derive(Debug, Clone)]
pub struct Polygon {
    pub points: Vec<Pt>,
}
