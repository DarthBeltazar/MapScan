import 'package:flutter/services.dart' show rootBundle;
import 'package:flutter_test/flutter_test.dart';
import 'package:app/src/rust/api/analyze.dart';
import 'package:app/src/rust/frb_generated.dart';
import 'package:integration_test/integration_test.dart';

// Exercises the real on-device FFI/OpenCV path (not the Windows desktop
// build tests/analyze.rs already covers) - proves rust_core.so actually
// loads libopencv_java4.so/libc++_shared.so and analyze_map runs to
// completion on-device, not just that the APK builds.
void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  setUpAll(() async => await RustLib.init());

  testWidgets('analyzeMap runs on-device against a real photo',
      (WidgetTester tester) async {
    final bytes =
        (await rootBundle.load('assets/test_fixtures/map0_small.jpg'))
            .buffer
            .asUint8List();

    final result = await analyzeMap(
      imageBytes: bytes,
      legendBoxes: const [],
      houghParam2: 38.0,
      sourceFilename: 'map0_small.jpg',
    );

    expect(result.width, greaterThan(0));
    expect(result.height, greaterThan(0));
    expect(result.geojson, contains('FeatureCollection'));
  });
}
