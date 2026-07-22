# app — Phase 1 client (Flutter + Rust)

The production mobile app: Flutter (Dart) UI, with CV/path-finding logic in a Rust
core crate (`rust/`) called via `flutter_rust_bridge`. Not a port of
[`../python_prototype/`](../python_prototype/) — see [`../PHASE0_HANDOFF.md`](../PHASE0_HANDOFF.md)
for what carries over conceptually versus what's photo-specific calibration that
doesn't.

For the full setup narrative (toolchain versions, every environment variable and why,
every non-obvious build/link bug hit on this machine and how it was fixed) see
[`../CLAUDE.md`](../CLAUDE.md)'s "`app/`" section — this file is a short pointer, not a
replacement for it.

## Layout

- `lib/` — Flutter/Dart UI (`main.dart`: photo picker, analysis overlay, manual
  correction, GeoJSON export).
- `rust/` — the `rust_core` crate: perspective correction, segmentation, course
  detection, cost-grid, pathfinding, GeoJSON export. Uses OpenCV via the `opencv`
  crate (real C++ FFI bindings, not a reimplementation).
- `rust_builder/` — `flutter_rust_bridge`'s cargokit build glue (vendored, with a
  couple of hand-patches — see `CLAUDE.md` before regenerating this directory).
- `integration_test/` — on-device tests, including one that exercises the real
  Android FFI → OpenCV path against a bundled test photo.

## Prerequisites

- Flutter SDK (stable channel), Rust via rustup, Android SDK/NDK.
- OpenCV (Windows desktop build + Android OpenCV SDK), LLVM/libclang for `bindgen`.
- Required environment variables (`OPENCV_LINK_LIBS`, `OPENCV_LINK_PATHS`,
  `OPENCV_INCLUDE_PATHS`, `OPENCV_BIN_DIR`, `OPENCV_ANDROID_SDK_PATH`,
  `LIBCLANG_PATH`) — see `CLAUDE.md` for exact values and why each one is needed.

## Building

```
flutter build windows
flutter build apk --debug
```

Both need the environment variables above set, and the Android build needs
`C:\Program Files\LLVM\bin` on `PATH` (not just `LIBCLANG_PATH`) for the `opencv`
crate's own build script to run — see `CLAUDE.md` if this fails with
`STATUS_DLL_NOT_FOUND`.

## Testing

```
cd rust && cargo test
flutter test integration_test/analyze_android_test.dart -d <device-id>
```

The Rust crate's tests run against the real in-scope test photos in
`../python_prototype/testData/` — no synthetic fixtures for the CV pipeline itself,
per this project's "don't fabricate validated data" rule (see root `CLAUDE.md`).
