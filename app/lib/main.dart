import 'dart:convert';
import 'dart:io';
import 'dart:math';

import 'package:file_picker/file_picker.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:app/src/rust/api/analyze.dart';
import 'package:app/src/rust/api/course_detection.dart';
import 'package:app/src/rust/api/geometry.dart';
import 'package:app/src/rust/api/pathfinding.dart';
import 'package:app/src/rust/api/segmentation.dart';
import 'package:app/src/rust/frb_generated.dart';

Future<void> main() async {
  await RustLib.init();
  runApp(const MyApp());
}

class MyApp extends StatelessWidget {
  const MyApp({super.key});

  @override
  Widget build(BuildContext context) {
    return const MaterialApp(
      home: AnalyzeScreen(),
    );
  }
}

// Excludes touch from the scrollables' own drag-to-scroll recognizer in the
// analysis viewer, so it doesn't fight the GestureDetector there for the
// same finger (see the `ScrollConfiguration` call site). Mouse/stylus/
// trackpad drag and the visible Scrollbar thumbs are unaffected.
class _NoTouchDragScrollBehavior extends MaterialScrollBehavior {
  @override
  Set<PointerDeviceKind> get dragDevices => const {
        PointerDeviceKind.mouse,
        PointerDeviceKind.stylus,
        PointerDeviceKind.trackpad,
      };
}

// Mirrors the Rust side's `analyze::HOUGH_PARAM2_DEFAULT` -- plain consts
// aren't bridged by flutter_rust_bridge, so this is kept in sync by hand.
// Deliberately *not* a per-file calibration table (see `ExcludeBox`'s doc
// comment on the Rust side): this demo screen runs on whatever photo the
// user picks, with no per-photo tuning, which is the honest "uncalibrated"
// experience a real user gets before any manual-correction UI exists.
const double _defaultHoughParam2 = 38.0;

class AnalyzeScreen extends StatefulWidget {
  const AnalyzeScreen({super.key});

  @override
  State<AnalyzeScreen> createState() => _AnalyzeScreenState();
}

/// Layer keys used by the visibility filter chips and the painter -- terrain
/// class names come straight from Rust's `ClassPolygons.className`, plus a
/// few fixed non-terrain layers (paths/legs/controls/route).
const List<String> _terrainLayerKeys = ['water', 'out_of_bounds', 'rock', 'marsh', 'forest', 'clearing', 'thicket'];
const List<String> _otherLayerKeys = ['paths', 'legs', 'controls', 'route'];

/// Manual-correction UI: this is `prompt.txt`'s hard requirement ("add/move/
/// delete a control, recolor a terrain area") that nothing in this repo
/// implements yet -- automatic detection is explicitly "best-effort, human
/// cleans up downstream" throughout the Rust port (see CLAUDE.md), and this
/// is that downstream cleanup step.
///
/// `start`/`finish` are modeled as the same marker kind as an ordinary
/// control (just visually distinct) rather than a separate data shape --
/// they're all "a point the user can add/move/delete", and `detect_start_
/// triangle` reliably returning `None` on every in-scope photo (see
/// CLAUDE.md) means letting a user manually place a start marker is not an
/// edge case, it's the common case.
enum _MarkerKind { control, start, finish }

enum _EditTool { moveDelete, addMarker, recolor }

class _EditableMarker {
  final int id;
  final _MarkerKind kind;
  final double x;
  final double y;
  final double radius;
  final String? code;

  const _EditableMarker({
    required this.id,
    required this.kind,
    required this.x,
    required this.y,
    this.radius = 10.0,
    this.code,
  });

  _EditableMarker moved(double nx, double ny) =>
      _EditableMarker(id: id, kind: kind, x: nx, y: ny, radius: radius, code: code);
}

class _EditablePolygon {
  final int id;
  final String className;
  final List<Pt> points;

  const _EditablePolygon({required this.id, required this.className, required this.points});

  _EditablePolygon recolored(String newClassName) =>
      _EditablePolygon(id: id, className: newClassName, points: points);
}

class _AnalyzeScreenState extends State<AnalyzeScreen> {
  Uint8List? _sourceBytes;
  AnalyzeResult? _result;
  bool _busy = false;
  String? _error;

  // Detection quality on a real, uncalibrated photo is genuinely noisy (see
  // CLAUDE.md: this is a faithful port of the Python prototype's own
  // "draft quality, human cleans up in the not-yet-built manual-correction
  // UI" state, not a rendering bug) -- these two controls let a user visually
  // isolate one layer at a time or hide small noise blobs, without touching
  // the underlying CV pipeline.
  // Replaced (not mutated) with a fresh Set on every toggle -- CustomPainter
  // .shouldRepaint compares old vs new instances by reference/value, so
  // mutating this Set in place would leave the *same* Set object behind and
  // silently fail to trigger a repaint when a layer is toggled.
  Set<String> _hiddenLayers = {};
  double _minPolygonAreaPx = 0.0;
  String? _sourceFilename;

  // Manual-correction state -- initialized from `_result` right after
  // analysis (see `_initEditableState`), then mutated only by the edit tools
  // below. `_result.segmentation`/`.course` themselves are never mutated:
  // they stay the honest "what detection actually found" record, and these
  // lists are the separate "what the user corrected it to" layer that
  // painting and GeoJSON export read from instead once analysis has run.
  List<_EditableMarker> _markers = [];
  List<_EditablePolygon> _polygons = [];
  int _nextEditId = 0;
  bool _editMode = false;
  _EditTool _tool = _EditTool.moveDelete;
  _MarkerKind _addKind = _MarkerKind.control;
  String _recolorTarget = _terrainLayerKeys.first;
  int? _selectedMarkerId;
  int? _draggingMarkerId;

  // Zoom: the working image (up to ~3000px wide) is downscaled to fit the
  // window by default (`_baseFitScale` below), which is fine for an overview
  // but too coarse to place a control precisely -- `_zoom` is a user-facing
  // multiplier on top of that fit scale, not a replacement for it. Panning
  // around a zoomed-in image uses ordinary scrollbars/trackpad/wheel
  // scrolling, not click-drag -- Flutter's default desktop `ScrollBehavior`
  // only treats touch/stylus drag as a scroll gesture, not mouse drag (see
  // `dragDevices`), so this doesn't fight the mouse-drag-to-move-a-marker
  // gesture already wired up below; both can coexist on the same canvas.
  static const double _minZoom = 0.5;
  static const double _maxZoom = 6.0;
  double _zoom = 1.0;
  // Zoom value at the start of the current pinch gesture -- ScaleUpdateDetails.scale
  // is cumulative relative to gesture start, not incremental, so `_setZoom` needs
  // this as its base rather than multiplying the live `_zoom` on every frame.
  double _scaleStartZoom = 1.0;
  // Native-image-space point the current pinch is anchored to, captured once at
  // gesture start -- keeps the same map point under the fingers as zoom changes,
  // instead of the content only ever growing from its top-left corner.
  Offset? _scaleFocalImagePt;
  final ScrollController _hScrollController = ScrollController();
  final ScrollController _vScrollController = ScrollController();
  // Captured at the top of the LayoutBuilder in the map viewer on every build,
  // so zoom-anchoring math is available to the +/-/Reset buttons too, which
  // live outside that LayoutBuilder's own scope.
  double _lastBaseScale = 1.0;
  Size _lastViewportSize = Size.zero;
  // Tracks pointer count across onScaleUpdate calls -- Flutter recomputes
  // the gesture's focal point from scratch whenever a finger joins/leaves
  // mid-gesture (e.g. the midpoint of 2 touches vs. a single remaining
  // touch's own position), which is a real, discontinuous jump, not motion.
  // Confirmed on-device: reacting to that one frame's `focalPointDelta` is
  // what caused the view to "teleport" right as fingers lifted off a pan.
  int _scaleLastPointerCount = 0;

  @override
  void dispose() {
    _hScrollController.dispose();
    _vScrollController.dispose();
    super.dispose();
  }

  void _setZoom(double z) => setState(() => _zoom = z.clamp(_minZoom, _maxZoom));

  // For adjustments that don't change content size (a pure pan) -- applied
  // immediately, since scroll extents are already correct and there's no
  // layout to wait for.
  void _scrollByNow(double ddx, double ddy) {
    if (_hScrollController.hasClients) {
      final pos = _hScrollController.position;
      _hScrollController.jumpTo((pos.pixels + ddx).clamp(pos.minScrollExtent, pos.maxScrollExtent));
    }
    if (_vScrollController.hasClients) {
      final pos = _vScrollController.position;
      _vScrollController.jumpTo((pos.pixels + ddy).clamp(pos.minScrollExtent, pos.maxScrollExtent));
    }
  }

  // Deferred a frame since maxScrollExtent only reflects a new zoom's
  // displayWidth/Height once that setState's rebuild has actually laid out.
  void _scrollBy(double ddx, double ddy) {
    WidgetsBinding.instance.addPostFrameCallback((_) => _scrollByNow(ddx, ddy));
  }

  // Native-image-space point currently at the centre of the viewport -- the
  // anchor the +/-/Reset buttons zoom around, since they have no finger
  // position to anchor to the way a pinch does.
  Offset _viewportCenterImagePt() {
    final scale = _lastBaseScale * _zoom;
    if (scale <= 0) return Offset.zero;
    final hOffset = _hScrollController.hasClients ? _hScrollController.position.pixels : 0.0;
    final vOffset = _vScrollController.hasClients ? _vScrollController.position.pixels : 0.0;
    return Offset(
      (hOffset + _lastViewportSize.width / 2) / scale,
      (vOffset + _lastViewportSize.height / 2) / scale,
    );
  }

  // Changes zoom while keeping `anchorImagePt` (a native-image-space point)
  // visually fixed under it, instead of the content only ever growing from
  // its top-left corner -- shared by the +/-/Reset buttons (anchored to the
  // viewport centre) and the pinch handler (anchored to the pinch centre).
  void _zoomAnchored(double newZoomRaw, Offset anchorImagePt) {
    final oldScale = _lastBaseScale * _zoom;
    final newZoom = newZoomRaw.clamp(_minZoom, _maxZoom);
    setState(() => _zoom = newZoom);
    final newScale = _lastBaseScale * newZoom;
    _scrollBy(anchorImagePt.dx * (newScale - oldScale), anchorImagePt.dy * (newScale - oldScale));
  }

  void _initEditableState(AnalyzeResult result) {
    var nextId = 0;
    final markers = <_EditableMarker>[];
    for (final c in result.course.controls) {
      markers.add(_EditableMarker(id: nextId++, kind: _MarkerKind.control, x: c.x, y: c.y, radius: c.radius, code: c.code));
    }
    final start = result.course.start;
    if (start != null) {
      markers.add(_EditableMarker(id: nextId++, kind: _MarkerKind.start, x: start.x, y: start.y));
    }
    final finish = result.course.finish;
    if (finish != null) {
      markers.add(_EditableMarker(id: nextId++, kind: _MarkerKind.finish, x: finish.x, y: finish.y));
    }
    final polygons = <_EditablePolygon>[];
    for (final classPolys in result.segmentation.polygonsByClass) {
      for (final poly in classPolys.polygons) {
        polygons.add(_EditablePolygon(id: nextId++, className: classPolys.className, points: poly.points));
      }
    }
    _markers = markers;
    _polygons = polygons;
    _nextEditId = nextId;
    _editMode = false;
    _tool = _EditTool.moveDelete;
    _selectedMarkerId = null;
    _draggingMarkerId = null;
    _zoom = 1.0;
  }

  /// Nearest marker whose hit radius contains `p` (image-space pixels),
  /// closest first -- a fixed minimum touch radius on top of the marker's
  /// own drawn radius, since a printed control circle can be a few px on a
  /// downscaled photo, too small to reliably tap otherwise.
  _EditableMarker? _markerAt(Offset p) {
    _EditableMarker? best;
    var bestD = double.infinity;
    for (final m in _markers) {
      final r = m.radius + 14.0;
      final d = (Offset(m.x, m.y) - p).distance;
      if (d <= r && d < bestD) {
        best = m;
        bestD = d;
      }
    }
    return best;
  }

  // Ray-casting point-in-polygon test on the (possibly self-intersecting,
  // draft-quality) simplified pixel-contour rings this pipeline produces --
  // good enough to pick "which polygon did the user tap", not a claim of
  // exact polygon validity (see segmentation.rs's module doc on why this
  // port doesn't carry a real polygon-validity concept).
  bool _pointInPolygon(Offset p, List<Pt> points) {
    var inside = false;
    for (var i = 0, j = points.length - 1; i < points.length; j = i++) {
      final pi = points[i], pj = points[j];
      final intersects = ((pi.y > p.dy) != (pj.y > p.dy)) &&
          (p.dx < (pj.x - pi.x) * (p.dy - pi.y) / (pj.y - pi.y) + pi.x);
      if (intersects) inside = !inside;
    }
    return inside;
  }

  _EditablePolygon? _polygonAt(Offset p) {
    for (final poly in _polygons.reversed) {
      if (poly.points.length >= 3 && _pointInPolygon(p, poly.points)) return poly;
    }
    return null;
  }

  /// A manually-added control needs to *look* like a real one, not a fixed
  /// small dot -- detected radii come from HoughCircles on a diagonal-scaled
  /// range (`course_detection.rs::detect_controls`, roughly 0.6%-2% of the
  /// image diagonal) and are routinely 20-70px on a ~3000px-wide working
  /// image, so a flat 10px default read as suspiciously tiny next to them.
  /// Match whatever real detected controls on *this* photo look like when
  /// there are any; only fall back to the diagonal-based estimate (the
  /// midpoint of that same Hough range) when there's nothing to match yet.
  double _defaultControlRadius() {
    final detected = _markers.where((m) => m.kind == _MarkerKind.control).toList();
    if (detected.isNotEmpty) {
      return detected.map((m) => m.radius).reduce((a, b) => a + b) / detected.length;
    }
    final result = _result;
    if (result != null) {
      final diag = sqrt((result.width * result.width + result.height * result.height).toDouble());
      // Midpoint of detect_controls' own minRadius/maxRadius fractions (0.6%/2% of diagonal).
      return diag * 0.013;
    }
    return 25.0;
  }

  void _handleTapUp(Offset imagePt) {
    if (!_editMode) return;
    switch (_tool) {
      case _EditTool.addMarker:
        setState(() {
          final markers = _markers.toList();
          // start/finish are singular markers -- adding a new one replaces
          // whichever one already existed rather than creating a second.
          if (_addKind != _MarkerKind.control) {
            markers.removeWhere((m) => m.kind == _addKind);
          }
          final radius = _addKind == _MarkerKind.control ? _defaultControlRadius() : 10.0;
          markers.add(_EditableMarker(id: _nextEditId++, kind: _addKind, x: imagePt.dx, y: imagePt.dy, radius: radius));
          _markers = markers;
        });
      case _EditTool.moveDelete:
        setState(() => _selectedMarkerId = _markerAt(imagePt)?.id);
      case _EditTool.recolor:
        final poly = _polygonAt(imagePt);
        if (poly != null) {
          setState(() {
            _polygons = _polygons.map((p) => p.id == poly.id ? p.recolored(_recolorTarget) : p).toList();
          });
        }
    }
  }

  void _handlePanStart(Offset imagePt) {
    if (!_editMode || _tool != _EditTool.moveDelete) return;
    final hit = _markerAt(imagePt);
    if (hit != null) {
      setState(() {
        _draggingMarkerId = hit.id;
        _selectedMarkerId = hit.id;
      });
    }
  }

  void _handlePanUpdate(Offset imagePt) {
    final draggingId = _draggingMarkerId;
    if (draggingId == null) return;
    setState(() {
      _markers = _markers.map((m) => m.id == draggingId ? m.moved(imagePt.dx, imagePt.dy) : m).toList();
    });
  }

  void _handlePanEnd() {
    _draggingMarkerId = null;
  }

  void _deleteSelectedMarker() {
    final id = _selectedMarkerId;
    if (id == null) return;
    setState(() {
      _markers = _markers.where((m) => m.id != id).toList();
      _selectedMarkerId = null;
    });
  }

  Future<void> _pickAndAnalyze() async {
    final picked = await FilePicker.platform.pickFiles(
      // FileType.image routes to Android's Photos picker, which only shows
      // MediaStore-indexed images - a photographed map saved to Downloads
      // (or pushed via adb, or exported from a scanner app) may never be
      // indexed there. FileType.custom uses the generic SAF document picker
      // instead, which can browse any folder.
      type: FileType.custom,
      allowedExtensions: const ['jpg', 'jpeg', 'png'],
      withData: true,
    );
    final bytes = picked?.files.single.bytes;
    if (bytes == null) return;
    final filename = picked!.files.single.name;

    setState(() {
      _sourceBytes = bytes;
      _sourceFilename = filename;
      _result = null;
      _error = null;
      _busy = true;
    });

    try {
      final result = await analyzeMap(
        imageBytes: bytes,
        legendBoxes: const [],
        houghParam2: _defaultHoughParam2,
        sourceFilename: filename,
      );
      setState(() {
        _result = result;
        _initEditableState(result);
      });
    } catch (e) {
      setState(() => _error = e.toString());
    } finally {
      setState(() => _busy = false);
    }
  }

  /// Builds the corrected `SegmentationResult`/`CourseResult` from the
  /// editable `_polygons`/`_markers` lists -- the manual-correction layer --
  /// and asks Rust to rebuild the GeoJSON from them via `rebuildGeojson`
  /// (see that function's doc comment for why route/terrain-breakdown pass
  /// through unchanged). `scale_to_original_photo` isn't carried on
  /// `AnalyzeResult` itself (only baked into the original `geojson` string at
  /// analysis time), so it's read back out of that string rather than
  /// duplicating the field on the Rust side for one export-time value.
  _EditableMarker? _firstMarkerOf(_MarkerKind kind) {
    for (final m in _markers) {
      if (m.kind == kind) return m;
    }
    return null;
  }

  Future<String> _correctedGeojson(AnalyzeResult result) async {
    final byClass = <String, List<Polygon>>{};
    for (final p in _polygons) {
      byClass.putIfAbsent(p.className, () => []).add(Polygon(points: p.points));
    }
    final segmentation = SegmentationResult(
      polygonsByClass: byClass.entries.map((e) => ClassPolygons(className: e.key, polygons: e.value)).toList(),
      paths: result.segmentation.paths,
    );

    final controls = _markers
        .where((m) => m.kind == _MarkerKind.control)
        .map((m) => Control(x: m.x, y: m.y, radius: m.radius, code: m.code))
        .toList();
    final start = _firstMarkerOf(_MarkerKind.start);
    final finish = _firstMarkerOf(_MarkerKind.finish);
    final course = CourseResult(
      controls: controls,
      start: start == null ? null : Pt(x: start.x, y: start.y),
      finish: finish == null ? null : Pt(x: finish.x, y: finish.y),
      legs: result.course.legs,
    );

    final originalProps = (jsonDecode(result.geojson) as Map<String, dynamic>)['properties'] as Map<String, dynamic>;
    final scaleToOriginal = (originalProps['scale_to_original_photo'] as num).toDouble();

    return rebuildGeojson(
      segmentation: segmentation,
      course: course,
      route: result.route,
      routeTerrainBreakdown: result.routeTerrainBreakdown,
      width: result.width,
      height: result.height,
      scaleToOriginal: scaleToOriginal,
      mnLineSpacingPx: result.mnLineSpacingPx,
      quadFound: result.quadFound,
      sourceFilename: _sourceFilename ?? '',
    );
  }

  Future<void> _saveGeoJson() async {
    final result = _result;
    if (result == null) return;
    final stem = (_sourceFilename ?? 'map').replaceAll(RegExp(r'\.[^.]+$'), '');
    final path = await FilePicker.platform.saveFile(
      dialogTitle: 'Save GeoJSON',
      fileName: '$stem.geojson',
      type: FileType.custom,
      allowedExtensions: ['geojson'],
    );
    if (path == null) return;
    final geojson = await _correctedGeojson(result);
    await File(path).writeAsString(geojson);
  }

  @override
  Widget build(BuildContext context) {
    final result = _result;
    return Scaffold(
      appBar: AppBar(title: const Text('Map analysis (preprocessing + segmentation + course)')),
      body: Center(
        child: SingleChildScrollView(
          padding: const EdgeInsets.all(16),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              ElevatedButton(
                onPressed: _busy ? null : _pickAndAnalyze,
                child: Text(_busy ? 'Working...' : 'Pick a map photo'),
              ),
              const SizedBox(height: 16),
              if (_error != null)
                Text('Error: $_error', style: const TextStyle(color: Colors.red)),
              if (result != null) ...[
                Text(
                  result.quadFound
                      ? 'Paper quad found -- rectified ${result.width}x${result.height}'
                      : 'No confident paper quad -- showing downscaled original '
                          '${result.width}x${result.height}',
                ),
                Text(
                  result.mnLineXs.isEmpty
                      ? 'No magnetic-north lines detected (best-effort, often empty)'
                      : 'Magnetic-north lines: ${result.mnLineXs.length} found'
                          '${result.mnLineSpacingPx != null ? ', spacing ${result.mnLineSpacingPx!.toStringAsFixed(1)}px' : ''}',
                ),
                Text(
                  'Course: ${result.course.controls.length} controls, '
                  '${result.course.legs.length} legs, '
                  '${result.course.start != null ? "start found" : "no start"}, '
                  '${result.course.finish != null ? "finish found" : "no finish"} '
                  '(draft detection, uncalibrated for this photo -- see legend below)',
                ),
                Text(
                  'Segmentation (polygon count per class, 0 means that class genuinely wasn\'t detected): '
                  '${result.segmentation.polygonsByClass.map((c) => "${c.className}=${c.polygons.length}").join(", ")}, '
                  'paths=${result.segmentation.paths.length} segments',
                ),
                Text(
                  result.route == null
                      ? 'Demo route: skipped (need a start + control, or 2+ controls)'
                      : 'Demo route: ${result.route!.points.length} points, cost ${result.route!.cost.toStringAsFixed(1)} '
                          '(cross-check ${(result.route!.cost - result.route!.recomputedCost).abs() < result.route!.cost.abs() * 0.01 ? "ok" : "MISMATCH"}), '
                          'terrain: ${result.routeTerrainBreakdown.map((f) => "${f.className}=${(f.fraction * 100).toStringAsFixed(0)}%").join(", ")} '
                          '-- connectivity proof, not a real course route',
                ),
                Wrap(
                  crossAxisAlignment: WrapCrossAlignment.center,
                  spacing: 8,
                  children: [
                    Text('GeoJSON: ${result.geojson.length} bytes (raw detection)'),
                    TextButton(onPressed: _saveGeoJson, child: const Text('Save GeoJSON (with corrections)...')),
                  ],
                ),
                const SizedBox(height: 12),
                _EditToolbar(
                  editMode: _editMode,
                  tool: _tool,
                  addKind: _addKind,
                  recolorTarget: _recolorTarget,
                  hasSelection: _selectedMarkerId != null,
                  onToggleEditMode: () => setState(() {
                    _editMode = !_editMode;
                    _selectedMarkerId = null;
                  }),
                  onToolChanged: (t) => setState(() {
                    _tool = t;
                    _selectedMarkerId = null;
                  }),
                  onAddKindChanged: (k) => setState(() => _addKind = k),
                  onRecolorTargetChanged: (c) => setState(() => _recolorTarget = c),
                  onDeleteSelected: _deleteSelectedMarker,
                ),
                const SizedBox(height: 12),
                _LayerFilterBar(
                  polygons: _polygons,
                  markers: _markers,
                  pathCount: result.segmentation.paths.length,
                  legCount: result.course.legs.length,
                  hasRoute: result.route != null,
                  hiddenLayers: _hiddenLayers,
                  minPolygonAreaPx: _minPolygonAreaPx,
                  onToggleLayer: (key) => setState(() {
                    _hiddenLayers = _hiddenLayers.contains(key)
                        ? (_hiddenLayers.toSet()..remove(key))
                        : (_hiddenLayers.toSet()..add(key));
                  }),
                  onMinAreaChanged: (v) => setState(() => _minPolygonAreaPx = v),
                ),
                const SizedBox(height: 12),
                Wrap(
                  crossAxisAlignment: WrapCrossAlignment.center,
                  spacing: 4,
                  children: [
                    IconButton(
                      tooltip: 'Zoom out',
                      icon: const Icon(Icons.zoom_out),
                      onPressed: _zoom > _minZoom
                          ? () => _zoomAnchored(_zoom / 1.25, _viewportCenterImagePt())
                          : null,
                    ),
                    SizedBox(width: 56, child: Text('${(_zoom * 100).round()}%', textAlign: TextAlign.center)),
                    IconButton(
                      tooltip: 'Zoom in',
                      icon: const Icon(Icons.zoom_in),
                      onPressed: _zoom < _maxZoom
                          ? () => _zoomAnchored(_zoom * 1.25, _viewportCenterImagePt())
                          : null,
                    ),
                    TextButton(
                      onPressed: () => _zoomAnchored(1.0, _viewportCenterImagePt()),
                      child: const Text('Reset'),
                    ),
                    const SizedBox(width: 8),
                    // Ctrl+scroll only exists with a physical keyboard/mouse -- on a
                    // touch device that hint is not just unhelpful but never true, so
                    // it's swapped for the touch equivalent (pinch, wired up below)
                    // instead of always showing the desktop-oriented text.
                    Text(
                      (Platform.isAndroid || Platform.isIOS)
                          ? 'Pinch to zoom; drag scrollbars to pan.'
                          : 'Ctrl + scroll wheel also zooms; scrollbars/trackpad pan when zoomed in.',
                      style: const TextStyle(fontSize: 12, color: Colors.black54),
                    ),
                  ],
                ),
                const SizedBox(height: 8),
                LayoutBuilder(
                  builder: (context, constraints) {
                    // The working-resolution image (up to ~3000px wide) is
                    // routinely wider than the window, so it's downscaled to
                    // fit by default (`baseScale`) -- Image.memory does that
                    // on its own (via its default BoxFit.scaleDown) whenever
                    // explicit width/height are smaller than the source, but
                    // CustomPaint does NOT rescale its drawing commands to
                    // match a shrunk canvas -- painting with raw
                    // full-resolution coordinates against a shrunk canvas is
                    // exactly what made the overlay drift away from the
                    // image. Computing one shared `scale` here (fit scale
                    // times the user's zoom level) and applying it to both
                    // the displayed image size AND the painter's canvas
                    // transform (see _AnalysisOverlayPainter.paint) keeps
                    // them in lockstep at any window size or zoom level.
                    final baseScale = (constraints.maxWidth / result.width).clamp(0.0, 1.0);
                    final scale = baseScale * _zoom;
                    final displayWidth = result.width * scale;
                    final displayHeight = result.height * scale;
                    // Gesture coordinates come in display (scaled) space;
                    // dividing by `scale` converts them back to the native
                    // image-pixel space every editable marker/polygon is
                    // stored in, same convention the painter itself un-scales
                    // via `canvas.scale(scale)` below.
                    Offset toImageSpace(Offset local) => scale > 0 ? local / scale : local;
                    final viewportHeight = min(700.0, MediaQuery.sizeOf(context).height * 0.7);
                    _lastBaseScale = baseScale;
                    _lastViewportSize = Size(constraints.maxWidth, viewportHeight);
                    return ScrollConfiguration(
                      // On touch devices, SingleChildScrollView's own drag-to-scroll
                      // recognizer and the GestureDetector below's scale recognizer
                      // both want the same finger -- confirmed directly on a real
                      // phone: dragging a selected marker scrolled the page instead of
                      // moving it, because the ancestor scroll view was winning the
                      // gesture arena. Excluding touch from `dragDevices` here stops
                      // these scroll views from claiming touch drags at all, leaving
                      // the GestureDetector as sole claimant (marker drag with one
                      // finger, pinch-zoom with two); mouse/stylus/trackpad drag -- and
                      // the visible Scrollbar thumbs, which aren't gated by
                      // `dragDevices` at all -- still pan normally.
                      behavior: _NoTouchDragScrollBehavior(),
                      child: Listener(
                        // Ctrl+wheel zooms (centered on the cursor is a nice-to-
                        // have skipped here, plain zoom is enough); a plain
                        // wheel/trackpad scroll is left alone so it keeps
                        // scrolling the nested scroll views below instead.
                        onPointerSignal: (event) {
                          if (event is PointerScrollEvent && HardwareKeyboard.instance.isControlPressed) {
                            _setZoom(_zoom * (event.scrollDelta.dy < 0 ? 1.1 : 1 / 1.1));
                          }
                        },
                        child: SizedBox(
                          height: viewportHeight,
                          child: Scrollbar(
                          controller: _vScrollController,
                          thumbVisibility: true,
                          notificationPredicate: (n) => n.depth == 0,
                          child: SingleChildScrollView(
                            controller: _vScrollController,
                            child: Scrollbar(
                              controller: _hScrollController,
                              thumbVisibility: true,
                              notificationPredicate: (n) => n.depth == 0,
                              child: SingleChildScrollView(
                                controller: _hScrollController,
                                scrollDirection: Axis.horizontal,
                                child: SizedBox(
                                  width: displayWidth,
                                  height: displayHeight,
                                  child: GestureDetector(
                                    behavior: HitTestBehavior.opaque,
                                    onTapUp: (details) => _handleTapUp(toImageSpace(details.localPosition)),
                                    // Scale (not Pan) handlers, so a 2-finger pinch on
                                    // touch devices zooms -- GestureDetector treats pan
                                    // and scale recognizers as mutually exclusive
                                    // (providing both throws), and Scale's callbacks
                                    // already cover the 1-finger case too via
                                    // `pointerCount`.
                                    onScaleStart: (details) {
                                      _scaleStartZoom = _zoom;
                                      _scaleFocalImagePt = toImageSpace(details.localFocalPoint);
                                      _scaleLastPointerCount = details.pointerCount;
                                      _handlePanStart(toImageSpace(details.localFocalPoint));
                                    },
                                    onScaleUpdate: (details) {
                                      final pointerCountChanged = details.pointerCount != _scaleLastPointerCount;
                                      _scaleLastPointerCount = details.pointerCount;
                                      if (details.pointerCount > 1) {
                                        if (pointerCountChanged) {
                                          // A finger just joined or left mid-gesture -- Flutter recomputes
                                          // the focal point from scratch for the new pointer set (e.g. the
                                          // midpoint of 2 touches vs. a single remaining touch), which is a
                                          // discontinuous jump, not real motion. Skip this one frame rather
                                          // than applying it as a scroll delta.
                                          return;
                                        }
                                        final newZoomRaw = _scaleStartZoom * details.scale;
                                        // A genuine 2-finger drag (not a pinch) still reports `scale` away
                                        // from 1.0 by small fractions every single frame -- real fingers
                                        // drift apart/together slightly during a deliberate pan -- and
                                        // reacting to that noise every frame (rebuild + deferred scroll
                                        // correction) is what caused the visible shake/jitter, confirmed
                                        // on-device. Below this threshold, treat it as pure pan: no zoom,
                                        // no rebuild, just move the view with the fingers.
                                        if ((newZoomRaw - _zoom).abs() > _zoom * 0.01) {
                                          final anchor =
                                              _scaleFocalImagePt ?? toImageSpace(details.localFocalPoint);
                                          _zoomAnchored(newZoomRaw, anchor);
                                        }
                                        // Applied immediately (not deferred): unlike the zoom-anchor
                                        // correction above, a pure translation doesn't change content
                                        // size, so scroll extents are already correct -- deferring this
                                        // too was adding a needless extra frame of lag on top of the
                                        // jitter fixed above.
                                        _scrollByNow(-details.focalPointDelta.dx, -details.focalPointDelta.dy);
                                      } else {
                                        _handlePanUpdate(toImageSpace(details.localFocalPoint));
                                      }
                                    },
                                    onScaleEnd: (_) {
                                      _scaleFocalImagePt = null;
                                      _handlePanEnd();
                                    },
                                    child: Stack(
                                      children: [
                                        // `fit: BoxFit.fill` is required, not cosmetic: the default
                                        // `BoxFit.scaleDown` never scales *up* past the image's native
                                        // pixel size, so once zoom pushes displayWidth/Height past
                                        // result.width/height, the bitmap would stop growing and sit
                                        // centered inside a box that keeps growing -- while CustomPaint's
                                        // `canvas.scale` below has no such cap and keeps scaling the
                                        // overlay correctly, which is exactly what made the two drift
                                        // apart (worse away from center, i.e. toward the sheet's edges).
                                        // width/height here are already the exact target box, so `fill`
                                        // (vs. `contain`) is safe: aspect ratio is preserved by construction
                                        // since both dimensions come from the same `scale` factor.
                                        Image.memory(
                                          result.imagePng,
                                          width: displayWidth,
                                          height: displayHeight,
                                          fit: BoxFit.fill,
                                        ),
                                        CustomPaint(
                                          size: Size(displayWidth, displayHeight),
                                          painter: _AnalysisOverlayPainter(
                                            polygons: _polygons,
                                            markers: _markers,
                                            paths: result.segmentation.paths,
                                            legs: result.course.legs,
                                            route: result.route,
                                            selectedMarkerId: _selectedMarkerId,
                                            scale: scale,
                                            hiddenLayers: _hiddenLayers,
                                            minPolygonAreaPx: _minPolygonAreaPx,
                                          ),
                                        ),
                                      ],
                                    ),
                                  ),
                                ),
                              ),
                            ),
                          ),
                        ),
                      ),
                    ),
                  );
                  },
                ),
                const SizedBox(height: 8),
                const _Legend(),
              ] else if (_sourceBytes != null && !_busy)
                Image.memory(_sourceBytes!, height: 200),
            ],
          ),
        ),
      ),
    );
  }
}

class _Legend extends StatelessWidget {
  const _Legend();

  @override
  Widget build(BuildContext context) {
    Widget row(Color color, String label) => Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Container(width: 14, height: 14, color: color),
            const SizedBox(width: 4),
            Text(label),
          ],
        );
    return Wrap(
      spacing: 12,
      runSpacing: 4,
      children: [
        row(Colors.blue, 'water'),
        row(Colors.purple, 'out_of_bounds'),
        row(Colors.brown, 'rock'),
        row(Colors.teal, 'marsh'),
        row(Colors.green, 'forest'),
        row(Colors.lightGreen, 'clearing'),
        row(Colors.green.shade900, 'thicket'),
        row(Colors.black, 'paths'),
        row(Colors.orange, 'course legs'),
        row(Colors.red, 'controls / start / finish'),
        row(Colors.yellow.shade700, 'demo route (connectivity proof only)'),
      ],
    );
  }
}

/// Lets a user isolate one layer at a time or hide small noise polygons --
/// detection quality on a real, uncalibrated photo is genuinely noisy (paths
/// in particular routinely pick up contour lines and vegetation-symbol edges,
/// not just real trails), and toggling layers on/off is how to tell "is this
/// real signal or clutter" without re-running the CV pipeline.
class _LayerFilterBar extends StatelessWidget {
  final List<_EditablePolygon> polygons;
  final List<_EditableMarker> markers;
  final int pathCount;
  final int legCount;
  final bool hasRoute;
  final Set<String> hiddenLayers;
  final double minPolygonAreaPx;
  final void Function(String key) onToggleLayer;
  final void Function(double value) onMinAreaChanged;

  const _LayerFilterBar({
    required this.polygons,
    required this.markers,
    required this.pathCount,
    required this.legCount,
    required this.hasRoute,
    required this.hiddenLayers,
    required this.minPolygonAreaPx,
    required this.onToggleLayer,
    required this.onMinAreaChanged,
  });

  int _countFor(String key) {
    switch (key) {
      case 'paths':
        return pathCount;
      case 'legs':
        return legCount;
      case 'controls':
        return markers.where((m) => m.kind == _MarkerKind.control).length;
      case 'route':
        return hasRoute ? 1 : 0;
      default:
        return polygons.where((p) => p.className == key).length;
    }
  }

  @override
  Widget build(BuildContext context) {
    Widget chip(String key) {
      final count = _countFor(key);
      return FilterChip(
        label: Text('$key ($count)'),
        selected: !hiddenLayers.contains(key),
        // A layer with nothing detected is still shown (greyed out, not
        // hidden from the list) -- seeing "out_of_bounds (0)" as an explicit,
        // untoggleable-feeling-different chip is exactly the "what was
        // actually detected" clarity that was missing before.
        onSelected: count == 0 ? null : (_) => onToggleLayer(key),
      );
    }

    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        const Text('Layers (tap to show/hide):'),
        const SizedBox(height: 4),
        Wrap(
          spacing: 6,
          runSpacing: 6,
          children: [..._terrainLayerKeys.map(chip), ..._otherLayerKeys.map(chip)],
        ),
        const SizedBox(height: 8),
        Row(
          children: [
            const Text('Hide polygons smaller than:'),
            Expanded(
              child: Slider(
                value: minPolygonAreaPx,
                min: 0,
                max: 5000,
                divisions: 50,
                label: '${minPolygonAreaPx.round()} px²',
                onChanged: onMinAreaChanged,
              ),
            ),
            SizedBox(width: 70, child: Text('${minPolygonAreaPx.round()} px²')),
          ],
        ),
      ],
    );
  }
}

/// The manual-correction toolbar: `prompt.txt`'s hard requirement, not a
/// nice-to-have (see CLAUDE.md/PHASE0_HANDOFF.md -- nothing in this repo
/// implemented this before now). Three tools, deliberately not a single
/// mixed-mode canvas: move/delete (tap-select + drag, then a delete button),
/// add a marker (pick a kind, then tap to place), and recolor (pick a target
/// terrain class, then tap a polygon to reassign it) -- each tap only ever
/// does one unambiguous thing per tool, rather than trying to infer "did the
/// user mean add/move/recolor" from a single universal gesture.
class _EditToolbar extends StatelessWidget {
  final bool editMode;
  final _EditTool tool;
  final _MarkerKind addKind;
  final String recolorTarget;
  final bool hasSelection;
  final VoidCallback onToggleEditMode;
  final void Function(_EditTool tool) onToolChanged;
  final void Function(_MarkerKind kind) onAddKindChanged;
  final void Function(String className) onRecolorTargetChanged;
  final VoidCallback onDeleteSelected;

  const _EditToolbar({
    required this.editMode,
    required this.tool,
    required this.addKind,
    required this.recolorTarget,
    required this.hasSelection,
    required this.onToggleEditMode,
    required this.onToolChanged,
    required this.onAddKindChanged,
    required this.onRecolorTargetChanged,
    required this.onDeleteSelected,
  });

  @override
  Widget build(BuildContext context) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Wrap(
          spacing: 8,
          runSpacing: 4,
          crossAxisAlignment: WrapCrossAlignment.center,
          children: [
            FilterChip(
              label: const Text('Manual correction'),
              avatar: const Icon(Icons.edit, size: 18),
              selected: editMode,
              onSelected: (_) => onToggleEditMode(),
            ),
            if (!editMode)
              const Text(
                'add/move/delete controls, recolor terrain -- prompt.txt\'s mandatory correction layer',
                style: TextStyle(fontStyle: FontStyle.italic, fontSize: 12),
              ),
          ],
        ),
        if (editMode) ...[
          const SizedBox(height: 8),
          Wrap(
            spacing: 8,
            runSpacing: 8,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              SegmentedButton<_EditTool>(
                segments: const [
                  ButtonSegment(value: _EditTool.moveDelete, label: Text('Move / delete'), icon: Icon(Icons.open_with)),
                  ButtonSegment(value: _EditTool.addMarker, label: Text('Add marker'), icon: Icon(Icons.add_location_alt)),
                  ButtonSegment(value: _EditTool.recolor, label: Text('Recolor area'), icon: Icon(Icons.format_color_fill)),
                ],
                selected: {tool},
                onSelectionChanged: (s) => onToolChanged(s.first),
              ),
              if (tool == _EditTool.moveDelete)
                IconButton(
                  tooltip: 'Delete selected marker',
                  icon: const Icon(Icons.delete),
                  onPressed: hasSelection ? onDeleteSelected : null,
                ),
              if (tool == _EditTool.addMarker)
                DropdownButton<_MarkerKind>(
                  value: addKind,
                  items: const [
                    DropdownMenuItem(value: _MarkerKind.control, child: Text('Control')),
                    DropdownMenuItem(value: _MarkerKind.start, child: Text('Start (replaces existing)')),
                    DropdownMenuItem(value: _MarkerKind.finish, child: Text('Finish (replaces existing)')),
                  ],
                  onChanged: (k) => k != null ? onAddKindChanged(k) : null,
                ),
              if (tool == _EditTool.recolor)
                DropdownButton<String>(
                  value: recolorTarget,
                  items: _terrainLayerKeys
                      .map((k) => DropdownMenuItem(value: k, child: Text(k)))
                      .toList(),
                  onChanged: (c) => c != null ? onRecolorTargetChanged(c) : null,
                ),
            ],
          ),
          const SizedBox(height: 4),
          Text(
            switch (tool) {
              _EditTool.moveDelete => 'Tap a marker to select it, drag to move it, or delete it with the trash button.',
              _EditTool.addMarker => 'Tap anywhere on the map to place a new $_addKindLabel.',
              _EditTool.recolor => 'Tap a terrain polygon to reclassify it as "$recolorTarget".',
            },
            style: const TextStyle(fontSize: 12, color: Colors.black54),
          ),
        ],
      ],
    );
  }

  String get _addKindLabel => switch (addKind) {
        _MarkerKind.control => 'control',
        _MarkerKind.start => 'start',
        _MarkerKind.finish => 'finish',
      };
}

class _AnalysisOverlayPainter extends CustomPainter {
  final List<_EditablePolygon> polygons;
  final List<_EditableMarker> markers;
  final List<Segment> paths;
  final List<Segment> legs;
  final RouteResult? route;
  final int? selectedMarkerId;
  /// Display size / native image size -- see the `LayoutBuilder` above for
  /// why this must be applied here too, not just to the displayed image.
  final double scale;
  final Set<String> hiddenLayers;
  final double minPolygonAreaPx;

  _AnalysisOverlayPainter({
    required this.polygons,
    required this.markers,
    required this.paths,
    required this.legs,
    required this.route,
    required this.selectedMarkerId,
    required this.scale,
    required this.hiddenLayers,
    required this.minPolygonAreaPx,
  });

  static const Map<String, Color> _classColors = {
    'water': Colors.blue,
    'out_of_bounds': Colors.purple,
    'rock': Colors.brown,
    'marsh': Colors.teal,
    'forest': Colors.green,
    'clearing': Colors.lightGreen,
  };

  Offset _pt(Pt p) => Offset(p.x, p.y);

  // Shoelace formula -- points are a simplified pixel-contour ring, same as
  // the Rust-side test helper this mirrors (app/rust/tests/analyze.rs).
  static double _polygonArea(List<Pt> points) {
    if (points.length < 3) return 0.0;
    var sum = 0.0;
    for (var i = 0; i < points.length; i++) {
      final a = points[i];
      final b = points[(i + 1) % points.length];
      sum += a.x * b.y - b.x * a.y;
    }
    return sum.abs() / 2.0;
  }

  @override
  void paint(Canvas canvas, Size size) {
    // Everything below draws using the *native* full-resolution pixel
    // coordinates that Rust reported (result.width/height space) -- this
    // single scale brings that in line with the actually-rendered (possibly
    // downscaled-to-fit-the-window) canvas size. `canvas.scale` also shrinks
    // stroke widths along with positions, though: on a heavily-downscaled
    // photo a strokeWidth of 1.5 could render at well under 1 physical
    // pixel, which is exactly why the overlay was hard to read. `w(px)`
    // below converts a *constant on-screen* pixel width into whatever
    // pre-scale width produces that after `canvas.scale` is applied, so
    // lines/markers stay legible regardless of window size or source photo
    // resolution.
    canvas.scale(scale);
    double w(double screenPx) => scale > 0 ? screenPx / scale : screenPx;

    for (final polygon in polygons) {
      if (hiddenLayers.contains(polygon.className)) continue;
      if (polygon.points.length < 2) continue;
      if (_polygonArea(polygon.points) < minPolygonAreaPx) continue;
      final color = _classColors[polygon.className] ?? Colors.green.shade900;
      final fillPaint = Paint()
        ..color = color.withValues(alpha: 0.35)
        ..style = PaintingStyle.fill;
      final strokePaint = Paint()
        ..color = color
        ..style = PaintingStyle.stroke
        ..strokeWidth = w(1.5);
      final path = Path()..moveTo(polygon.points.first.x, polygon.points.first.y);
      for (final p in polygon.points.skip(1)) {
        path.lineTo(p.x, p.y);
      }
      path.close();
      // Filled first: a thin outline alone is easy to miss against a busy
      // printed map background -- a translucent fill makes "this whole
      // area was classified as X" visible at a glance, which is what was
      // actually missing before, not just a color choice.
      canvas.drawPath(path, fillPaint);
      canvas.drawPath(path, strokePaint);
    }

    // Paths were drawn in black before, which is nearly invisible against
    // these maps' own printed black path ink -- cyan has no competing use
    // in the legend/map and reads clearly against both light and dark
    // background.
    if (!hiddenLayers.contains('paths')) {
      final pathPaint = Paint()
        ..color = Colors.cyanAccent.shade700
        ..strokeWidth = w(2.5)
        ..strokeCap = StrokeCap.round;
      for (final seg in paths) {
        canvas.drawLine(_pt(seg.a), _pt(seg.b), pathPaint);
      }
    }

    if (!hiddenLayers.contains('legs')) {
      final legPaint = Paint()
        ..color = Colors.orange
        ..strokeWidth = w(2.5)
        ..strokeCap = StrokeCap.round;
      for (final leg in legs) {
        canvas.drawLine(_pt(leg.a), _pt(leg.b), legPaint);
      }
    }

    if (!hiddenLayers.contains('controls')) {
      for (final m in markers) {
        final isSelected = m.id == selectedMarkerId;
        final markerColor = isSelected ? Colors.blueAccent : Colors.red;
        switch (m.kind) {
          case _MarkerKind.control:
            canvas.drawCircle(
              Offset(m.x, m.y),
              m.radius,
              Paint()
                ..color = markerColor
                ..style = PaintingStyle.stroke
                ..strokeWidth = w(isSelected ? 4.5 : 3.0),
            );
          case _MarkerKind.start:
            canvas.drawPath(
              _trianglePath(Offset(m.x, m.y), w(20)),
              Paint()
                ..color = markerColor
                ..style = PaintingStyle.fill,
            );
          case _MarkerKind.finish:
            canvas.drawCircle(
              Offset(m.x, m.y),
              w(14),
              Paint()
                ..color = markerColor
                ..style = PaintingStyle.fill,
            );
        }
        if (isSelected) {
          canvas.drawCircle(
            Offset(m.x, m.y),
            w(m.kind == _MarkerKind.control ? m.radius + 10 : 24),
            Paint()
              ..color = Colors.blueAccent
              ..style = PaintingStyle.stroke
              ..strokeWidth = w(1.5),
          );
        }
      }
    }

    final routeValue = route;
    if (!hiddenLayers.contains('route') && routeValue != null && routeValue.points.length >= 2) {
      final routePaint = Paint()
        ..color = Colors.yellow.shade700
        ..style = PaintingStyle.stroke
        ..strokeWidth = w(4.0)
        ..strokeCap = StrokeCap.round;
      final path = Path()..moveTo(routeValue.points.first.x, routeValue.points.first.y);
      for (final p in routeValue.points.skip(1)) {
        path.lineTo(p.x, p.y);
      }
      canvas.drawPath(path, routePaint);
    }
  }

  Path _trianglePath(Offset center, double size) {
    return Path()
      ..moveTo(center.dx, center.dy - size)
      ..lineTo(center.dx - size, center.dy + size)
      ..lineTo(center.dx + size, center.dy + size)
      ..close();
  }

  @override
  bool shouldRepaint(covariant _AnalysisOverlayPainter oldDelegate) =>
      !identical(oldDelegate.polygons, polygons) ||
      !identical(oldDelegate.markers, markers) ||
      oldDelegate.selectedMarkerId != selectedMarkerId ||
      oldDelegate.scale != scale ||
      oldDelegate.hiddenLayers != hiddenLayers ||
      oldDelegate.minPolygonAreaPx != minPolygonAreaPx;
}
