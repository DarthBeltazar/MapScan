import 'dart:io';
import 'dart:typed_data';

import 'package:file_picker/file_picker.dart';
import 'package:flutter/material.dart';
import 'package:app/src/rust/api/analyze.dart';
import 'package:app/src/rust/api/geometry.dart';
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

  Future<void> _pickAndAnalyze() async {
    final picked = await FilePicker.platform.pickFiles(
      type: FileType.image,
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
      setState(() => _result = result);
    } catch (e) {
      setState(() => _error = e.toString());
    } finally {
      setState(() => _busy = false);
    }
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
    await File(path).writeAsString(result.geojson);
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
                Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Text('GeoJSON: ${result.geojson.length} bytes'),
                    const SizedBox(width: 8),
                    TextButton(onPressed: _saveGeoJson, child: const Text('Save GeoJSON...')),
                  ],
                ),
                const SizedBox(height: 12),
                _LayerFilterBar(
                  result: result,
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
                LayoutBuilder(
                  builder: (context, constraints) {
                    // The working-resolution image (up to ~3000px wide) is
                    // routinely wider than the window, so it must be
                    // downscaled to fit. Image.memory does that on its own
                    // (via its default BoxFit.scaleDown) whenever explicit
                    // width/height are smaller than the source, but
                    // CustomPaint does NOT rescale its drawing commands to
                    // match a shrunk canvas -- painting with raw
                    // full-resolution coordinates against a shrunk canvas is
                    // exactly what made the overlay drift away from the
                    // image. Computing one shared `scale` here and applying
                    // it to both the displayed image size AND the painter's
                    // canvas transform (see _AnalysisOverlayPainter.paint)
                    // keeps them in lockstep at any window size.
                    final scale = (constraints.maxWidth / result.width).clamp(0.0, 1.0);
                    final displayWidth = result.width * scale;
                    final displayHeight = result.height * scale;
                    return SizedBox(
                      width: displayWidth,
                      height: displayHeight,
                      child: Stack(
                        children: [
                          Image.memory(result.imagePng, width: displayWidth, height: displayHeight),
                          CustomPaint(
                            size: Size(displayWidth, displayHeight),
                            painter: _AnalysisOverlayPainter(result, scale, _hiddenLayers, _minPolygonAreaPx),
                          ),
                        ],
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
  final AnalyzeResult result;
  final Set<String> hiddenLayers;
  final double minPolygonAreaPx;
  final void Function(String key) onToggleLayer;
  final void Function(double value) onMinAreaChanged;

  const _LayerFilterBar({
    required this.result,
    required this.hiddenLayers,
    required this.minPolygonAreaPx,
    required this.onToggleLayer,
    required this.onMinAreaChanged,
  });

  int _countFor(String key) {
    switch (key) {
      case 'paths':
        return result.segmentation.paths.length;
      case 'legs':
        return result.course.legs.length;
      case 'controls':
        return result.course.controls.length;
      case 'route':
        return result.route == null ? 0 : 1;
      default:
        return result.segmentation.polygonsByClass
            .firstWhere((c) => c.className == key, orElse: () => const ClassPolygons(className: '', polygons: []))
            .polygons
            .length;
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

class _AnalysisOverlayPainter extends CustomPainter {
  final AnalyzeResult result;
  /// Display size / native image size -- see the `LayoutBuilder` above for
  /// why this must be applied here too, not just to the displayed image.
  final double scale;
  final Set<String> hiddenLayers;
  final double minPolygonAreaPx;
  _AnalysisOverlayPainter(this.result, this.scale, this.hiddenLayers, this.minPolygonAreaPx);

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

    for (final classPolys in result.segmentation.polygonsByClass) {
      if (hiddenLayers.contains(classPolys.className)) continue;
      final color = _classColors[classPolys.className] ?? Colors.green.shade900;
      final fillPaint = Paint()
        ..color = color.withValues(alpha: 0.35)
        ..style = PaintingStyle.fill;
      final strokePaint = Paint()
        ..color = color
        ..style = PaintingStyle.stroke
        ..strokeWidth = w(1.5);
      for (final polygon in classPolys.polygons) {
        if (polygon.points.length < 2) continue;
        if (_polygonArea(polygon.points) < minPolygonAreaPx) continue;
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
      for (final seg in result.segmentation.paths) {
        canvas.drawLine(_pt(seg.a), _pt(seg.b), pathPaint);
      }
    }

    if (!hiddenLayers.contains('legs')) {
      final legPaint = Paint()
        ..color = Colors.orange
        ..strokeWidth = w(2.5)
        ..strokeCap = StrokeCap.round;
      for (final leg in result.course.legs) {
        canvas.drawLine(_pt(leg.a), _pt(leg.b), legPaint);
      }
    }

    if (!hiddenLayers.contains('controls')) {
      final controlPaint = Paint()
        ..color = Colors.red
        ..style = PaintingStyle.stroke
        ..strokeWidth = w(3.0);
      for (final c in result.course.controls) {
        canvas.drawCircle(Offset(c.x, c.y), c.radius, controlPaint);
      }

      final markerPaint = Paint()
        ..color = Colors.red
        ..style = PaintingStyle.fill;
      final start = result.course.start;
      if (start != null) {
        canvas.drawPath(_trianglePath(_pt(start), w(20)), markerPaint);
      }
      final finish = result.course.finish;
      if (finish != null) {
        canvas.drawCircle(_pt(finish), w(14), markerPaint);
      }
    }

    final route = result.route;
    if (!hiddenLayers.contains('route') && route != null && route.points.length >= 2) {
      final routePaint = Paint()
        ..color = Colors.yellow.shade700
        ..style = PaintingStyle.stroke
        ..strokeWidth = w(4.0)
        ..strokeCap = StrokeCap.round;
      final path = Path()..moveTo(route.points.first.x, route.points.first.y);
      for (final p in route.points.skip(1)) {
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
      oldDelegate.result != result ||
      oldDelegate.scale != scale ||
      oldDelegate.hiddenLayers != hiddenLayers ||
      oldDelegate.minPolygonAreaPx != minPolygonAreaPx;
}
