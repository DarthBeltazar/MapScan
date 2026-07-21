import 'dart:typed_data';

import 'package:file_picker/file_picker.dart';
import 'package:flutter/material.dart';
import 'package:app/src/rust/api/preprocessing.dart';
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
      home: RectifyScreen(),
    );
  }
}

class RectifyScreen extends StatefulWidget {
  const RectifyScreen({super.key});

  @override
  State<RectifyScreen> createState() => _RectifyScreenState();
}

class _RectifyScreenState extends State<RectifyScreen> {
  Uint8List? _sourceBytes;
  RectifyResult? _result;
  bool _busy = false;
  String? _error;

  Future<void> _pickAndRectify() async {
    final picked = await FilePicker.platform.pickFiles(
      type: FileType.image,
      withData: true,
    );
    final bytes = picked?.files.single.bytes;
    if (bytes == null) return;

    setState(() {
      _sourceBytes = bytes;
      _result = null;
      _error = null;
      _busy = true;
    });

    try {
      final result = await rectifyPhoto(imageBytes: bytes);
      setState(() => _result = result);
    } catch (e) {
      setState(() => _error = e.toString());
    } finally {
      setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: const Text('Perspective correction')),
      body: Center(
        child: SingleChildScrollView(
          padding: const EdgeInsets.all(16),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              ElevatedButton(
                onPressed: _busy ? null : _pickAndRectify,
                child: Text(_busy ? 'Working...' : 'Pick a map photo'),
              ),
              const SizedBox(height: 16),
              if (_error != null)
                Text('Error: $_error', style: const TextStyle(color: Colors.red)),
              if (_result != null) ...[
                Text(
                  _result!.quadFound
                      ? 'Paper quad found -- rectified ${_result!.width}x${_result!.height}'
                      : 'No confident paper quad -- showing downscaled original '
                          '${_result!.width}x${_result!.height}',
                ),
                const SizedBox(height: 12),
                Image.memory(_result!.imagePng),
              ] else if (_sourceBytes != null && !_busy)
                Image.memory(_sourceBytes!, height: 200),
            ],
          ),
        ),
      ),
    );
  }
}
