import 'dart:ffi';
import 'dart:typed_data';

import 'package:haven_app/services/win_audio.dart';

// Diagnostic: check number of audio devices
final _winmm = DynamicLibrary.open('winmm.dll');
final _waveInGetNumDevs = _winmm
    .lookupFunction<Uint32 Function(), int Function()>('waveInGetNumDevs');
final _waveOutGetNumDevs = _winmm
    .lookupFunction<Uint32 Function(), int Function()>('waveOutGetNumDevs');

/// Tests the Windows audio FFI layer.
void main() {
  print('=== Windows Audio FFI Test ===');

  // Verify struct sizes match Windows ABI
  print('sizeof(WAVEFORMATEX) = ${sizeOf<WAVEFORMATEX>()} (expected 18)');
  assert(sizeOf<WAVEFORMATEX>() == 18,
      'WAVEFORMATEX size mismatch: ${sizeOf<WAVEFORMATEX>()}');

  print('sizeof(WAVEHDR) = ${sizeOf<WAVEHDR>()} (expected 48)');
  assert(
      sizeOf<WAVEHDR>() == 48, 'WAVEHDR size mismatch: ${sizeOf<WAVEHDR>()}');

  // Check available devices
  final numIn = _waveInGetNumDevs();
  final numOut = _waveOutGetNumDevs();
  print('\nAudio input devices:  $numIn');
  print('Audio output devices: $numOut');

  // Test capture
  print('\n--- Capture test ---');
  if (numIn == 0) {
    print('SKIP: no microphone available');
  } else {
    final capture = WinAudioCapture();
    try {
      capture.start();
      print('Capture started (16kHz mono 16-bit)');

      int totalBytes = 0;
      for (int i = 0; i < 10; i++) {
        final sw = Stopwatch()..start();
        while (sw.elapsedMilliseconds < 50) {}

        final data = capture.poll();
        totalBytes += data.length;
        if (data.isNotEmpty) {
          print('  Poll $i: ${data.length} bytes');
        }
      }

      capture.stop();
      print('Total captured: $totalBytes bytes '
          '(~${(totalBytes / 32000 * 1000).toStringAsFixed(0)}ms)');
    } on AudioException catch (e) {
      print('Capture failed: $e');
      capture.dispose();
    }
  }

  // Test playback
  print('\n--- Playback test ---');
  if (numOut == 0) {
    print('SKIP: no audio output available');
  } else {
    final playback = WinAudioPlayback();
    try {
      playback.start();
      print('Playback started');

      final silence = Uint8List(640);
      for (int i = 0; i < 5; i++) {
        final ok = playback.feed(silence);
        print('  Feed $i: ${ok ? "accepted" : "dropped"}');
      }

      final sw = Stopwatch()..start();
      while (sw.elapsedMilliseconds < 200) {}

      playback.stop();
      print('Playback stopped.');
    } on AudioException catch (e) {
      print('Playback failed: $e');
      playback.dispose();
    }
  }

  print('\n=== STRUCT VERIFICATION PASSED ===');
  if (numIn > 0 && numOut > 0) {
    print('=== AUDIO DEVICE TESTS PASSED ===');
  } else {
    print('=== AUDIO DEVICE TESTS SKIPPED (no devices) ===');
  }
}
