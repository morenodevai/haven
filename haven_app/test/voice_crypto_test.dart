import 'dart:convert';
import 'dart:typed_data';

import 'package:haven_app/services/crypto_service.dart';

/// Verifies the voice-specific crypto (concatenated IV format) works correctly.
void main() {
  // Use the default Haven channel key
  const keyBase64 = 'QUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUE=';

  // Simulate a 20ms audio frame: 320 samples of Int16 silence
  final pcmData = Uint8List(640); // 320 * 2 bytes = 640

  // Fill with a simple waveform for testing
  final byteData = ByteData.view(pcmData.buffer);
  for (int i = 0; i < 320; i++) {
    // Simple sine-ish pattern to make it non-zero
    final sample = (i % 64 - 32) * 100;
    byteData.setInt16(i * 2, sample, Endian.little);
  }

  print('=== Voice Crypto Test ===');
  print('PCM data: ${pcmData.length} bytes (320 samples)');

  // Encrypt
  final encrypted = CryptoService.encryptVoiceSync(keyBase64, pcmData);
  print('Encrypted base64: ${encrypted.length} chars');

  // Verify format: base64 decodes to 12 (IV) + 640 (plaintext) + 16 (GCM tag) = 668 bytes
  final decoded = base64.decode(encrypted);
  print('Decoded bytes: ${decoded.length}');
  assert(decoded.length == 668, 'Expected 668 bytes, got ${decoded.length}');

  // First 12 bytes are the IV
  final iv = decoded.sublist(0, 12);
  print('IV: ${iv.map((b) => b.toRadixString(16).padLeft(2, "0")).join(" ")}');

  // Decrypt
  final decrypted = CryptoService.decryptVoiceSync(keyBase64, encrypted);
  assert(decrypted != null, 'Decryption returned null');
  assert(decrypted!.length == 640, 'Decrypted length: ${decrypted.length}');

  // Verify roundtrip
  final dec = decrypted!;
  for (int i = 0; i < 640; i++) {
    assert(dec[i] == pcmData[i],
        'Mismatch at byte $i: ${dec[i]} != ${pcmData[i]}');
  }
  print('Roundtrip: PASS (640 bytes match)');

  // Test that different encryptions produce different ciphertexts (random IV)
  final encrypted2 = CryptoService.encryptVoiceSync(keyBase64, pcmData);
  assert(encrypted != encrypted2, 'Two encryptions should differ (random IV)');
  print('Random IV: PASS (different each time)');

  // Both decrypt to the same plaintext
  final decrypted2 = CryptoService.decryptVoiceSync(keyBase64, encrypted2);
  assert(decrypted2 != null);
  for (int i = 0; i < 640; i++) {
    assert(decrypted2![i] == pcmData[i]);
  }
  print('Both decrypt correctly: PASS');

  // Test invalid data
  final badResult = CryptoService.decryptVoiceSync(keyBase64, 'invalid');
  assert(badResult == null, 'Invalid data should return null');
  print('Invalid data handling: PASS');

  // Performance test: encrypt 50 frames (1 second of audio)
  final sw = Stopwatch()..start();
  for (int i = 0; i < 50; i++) {
    CryptoService.encryptVoiceSync(keyBase64, pcmData);
  }
  sw.stop();
  print('Encrypt 50 frames: ${sw.elapsedMilliseconds}ms '
      '(${(sw.elapsedMilliseconds / 50).toStringAsFixed(1)}ms/frame)');

  // Decrypt performance
  sw.reset();
  sw.start();
  for (int i = 0; i < 50; i++) {
    CryptoService.decryptVoiceSync(keyBase64, encrypted);
  }
  sw.stop();
  print('Decrypt 50 frames: ${sw.elapsedMilliseconds}ms '
      '(${(sw.elapsedMilliseconds / 50).toStringAsFixed(1)}ms/frame)');

  print('\n=== ALL VOICE CRYPTO TESTS PASSED ===');
}
