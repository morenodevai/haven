import 'dart:convert';
import 'dart:isolate';
import 'dart:typed_data';

import 'package:pointycastle/export.dart';

/// AES-256-GCM encryption/decryption service.
///
/// Matches the Rust haven-crypto crate exactly:
/// - 32-byte key (AES-256)
/// - 12-byte random nonce
/// - GCM authentication tag appended to ciphertext
/// - All values base64-encoded for transport
class CryptoService {
  CryptoService._();

  static final _secureRandom = _initSecureRandom();

  static SecureRandom _initSecureRandom() {
    final random = FortunaRandom();
    final seed = Uint8List(32);
    final dartRandom = DateTime.now().microsecondsSinceEpoch;
    for (int i = 0; i < 32; i++) {
      seed[i] = ((dartRandom >> (i % 8)) ^ (i * 37)) & 0xFF;
    }
    random.seed(KeyParameter(seed));
    return random;
  }

  /// Encrypt plaintext string with AES-256-GCM.
  /// Returns {ciphertext: base64, nonce: base64}.
  static Future<Map<String, String>> encrypt(
    String keyBase64,
    String plaintext,
  ) async {
    return Isolate.run(() => _encryptSync(keyBase64, plaintext));
  }

  /// Decrypt ciphertext with AES-256-GCM.
  /// Returns the plaintext string.
  static Future<String> decrypt(
    String keyBase64,
    String ciphertextBase64,
    String nonceBase64,
  ) async {
    return Isolate.run(
        () => _decryptSync(keyBase64, ciphertextBase64, nonceBase64));
  }

  /// Synchronous encrypt (for use inside isolates).
  static Map<String, String> _encryptSync(
    String keyBase64,
    String plaintext,
  ) {
    final key = base64.decode(keyBase64);
    if (key.length != 32) {
      throw ArgumentError('Key must be 32 bytes, got ${key.length}');
    }

    final nonce = _generateNonce();
    final plaintextBytes = utf8.encode(plaintext);

    final cipher = GCMBlockCipher(AESEngine())
      ..init(
        true,
        AEADParameters(
          KeyParameter(Uint8List.fromList(key)),
          128, // 128-bit tag
          Uint8List.fromList(nonce),
          Uint8List(0), // no AAD
        ),
      );

    // Allocate buffer and track actual bytes written
    final outputBuf = Uint8List(cipher.getOutputSize(plaintextBytes.length));
    final len = cipher.processBytes(
        Uint8List.fromList(plaintextBytes), 0, plaintextBytes.length,
        outputBuf, 0);
    final finalLen = cipher.doFinal(outputBuf, len);

    // Only take the actual bytes: plaintext + 16-byte GCM tag
    final ciphertext = outputBuf.sublist(0, len + finalLen);

    return {
      'ciphertext': base64.encode(ciphertext),
      'nonce': base64.encode(nonce),
    };
  }

  /// Synchronous decrypt (for use inside isolates).
  static String _decryptSync(
    String keyBase64,
    String ciphertextBase64,
    String nonceBase64,
  ) {
    final key = base64.decode(keyBase64);
    if (key.length != 32) {
      throw ArgumentError('Key must be 32 bytes, got ${key.length}');
    }

    final ciphertext = base64.decode(ciphertextBase64);
    final nonce = base64.decode(nonceBase64);

    final cipher = GCMBlockCipher(AESEngine())
      ..init(
        false,
        AEADParameters(
          KeyParameter(Uint8List.fromList(key)),
          128,
          Uint8List.fromList(nonce),
          Uint8List(0),
        ),
      );

    // Allocate buffer and track actual bytes written
    final outputBuf = Uint8List(cipher.getOutputSize(ciphertext.length));
    final len = cipher.processBytes(
        Uint8List.fromList(ciphertext), 0, ciphertext.length, outputBuf, 0);
    final finalLen = cipher.doFinal(outputBuf, len);

    // Only take actual plaintext bytes (ciphertext minus 16-byte tag)
    final plaintext = outputBuf.sublist(0, len + finalLen);
    return utf8.decode(plaintext);
  }

  static List<int> _generateNonce() {
    final nonce = Uint8List(12);
    for (int i = 0; i < 12; i++) {
      nonce[i] = _secureRandom.nextUint8();
    }
    return nonce;
  }

  // ── File crypto ──
  //
  // Files are encrypted as: IV (12 bytes) + ciphertext + GCM tag (16 bytes)
  // Same concatenated format as voice, but runs in an Isolate for large files.

  /// Encrypt raw file bytes. Returns concatenated IV + ciphertext + tag.
  static Future<Uint8List> encryptFile(
      String keyBase64, Uint8List plainBytes) async {
    return Isolate.run(() => _encryptFileSync(keyBase64, plainBytes));
  }

  /// Decrypt file bytes. Input format: IV + ciphertext + tag.
  static Future<Uint8List> decryptFile(
      String keyBase64, Uint8List encryptedBytes) async {
    return Isolate.run(() => _decryptFileSync(keyBase64, encryptedBytes));
  }

  static Uint8List _encryptFileSync(String keyBase64, Uint8List plainBytes) {
    final key = base64.decode(keyBase64);
    final nonce = _generateNonce();

    final cipher = GCMBlockCipher(AESEngine())
      ..init(
        true,
        AEADParameters(
          KeyParameter(Uint8List.fromList(key)),
          128,
          Uint8List.fromList(nonce),
          Uint8List(0),
        ),
      );

    final outputBuf = Uint8List(cipher.getOutputSize(plainBytes.length));
    final len =
        cipher.processBytes(plainBytes, 0, plainBytes.length, outputBuf, 0);
    final finalLen = cipher.doFinal(outputBuf, len);
    final ciphertext = outputBuf.sublist(0, len + finalLen);

    final combined = Uint8List(12 + ciphertext.length);
    combined.setRange(0, 12, nonce);
    combined.setRange(12, combined.length, ciphertext);
    return combined;
  }

  static Uint8List _decryptFileSync(
      String keyBase64, Uint8List encryptedBytes) {
    final key = base64.decode(keyBase64);
    if (encryptedBytes.length < 29) {
      throw ArgumentError('Encrypted data too short');
    }

    final nonce = encryptedBytes.sublist(0, 12);
    final ciphertext = encryptedBytes.sublist(12);

    final cipher = GCMBlockCipher(AESEngine())
      ..init(
        false,
        AEADParameters(
          KeyParameter(Uint8List.fromList(key)),
          128,
          Uint8List.fromList(nonce),
          Uint8List(0),
        ),
      );

    final outputBuf = Uint8List(cipher.getOutputSize(ciphertext.length));
    final len =
        cipher.processBytes(ciphertext, 0, ciphertext.length, outputBuf, 0);
    final finalLen = cipher.doFinal(outputBuf, len);
    return outputBuf.sublist(0, len + finalLen);
  }

  // ── Voice-specific crypto ──
  //
  // Voice uses a single concatenated format: base64(IV + ciphertext + GCM tag)
  // This matches the existing Svelte client's voice encryption format.

  /// Encrypt raw PCM audio for voice transmission.
  /// Returns base64(12-byte-IV + ciphertext + 16-byte-GCM-tag).
  /// Runs synchronously — voice frames are small (~640 bytes), so this is fast.
  static String encryptVoiceSync(String keyBase64, Uint8List pcmData) {
    final key = base64.decode(keyBase64);
    final nonce = _generateNonce();

    final cipher = GCMBlockCipher(AESEngine())
      ..init(
        true,
        AEADParameters(
          KeyParameter(Uint8List.fromList(key)),
          128,
          Uint8List.fromList(nonce),
          Uint8List(0),
        ),
      );

    final outputBuf = Uint8List(cipher.getOutputSize(pcmData.length));
    final len = cipher.processBytes(pcmData, 0, pcmData.length, outputBuf, 0);
    final finalLen = cipher.doFinal(outputBuf, len);
    final ciphertext = outputBuf.sublist(0, len + finalLen);

    // Concatenate: IV (12) + ciphertext (includes appended GCM tag)
    final combined = Uint8List(12 + ciphertext.length);
    combined.setRange(0, 12, nonce);
    combined.setRange(12, combined.length, ciphertext);

    return base64.encode(combined);
  }

  /// Decrypt voice audio data.
  /// Input: base64(12-byte-IV + ciphertext + 16-byte-GCM-tag).
  /// Returns raw PCM bytes, or null on failure.
  static Uint8List? decryptVoiceSync(String keyBase64, String encryptedBase64) {
    try {
      final key = base64.decode(keyBase64);
      final combined = base64.decode(encryptedBase64);
      if (combined.length < 29) return null; // 12 IV + 16 tag + 1 byte min

      final nonce = combined.sublist(0, 12);
      final ciphertext = combined.sublist(12);

      final cipher = GCMBlockCipher(AESEngine())
        ..init(
          false,
          AEADParameters(
            KeyParameter(Uint8List.fromList(key)),
            128,
            Uint8List.fromList(nonce),
            Uint8List(0),
          ),
        );

      final outputBuf = Uint8List(cipher.getOutputSize(ciphertext.length));
      final len = cipher.processBytes(
          ciphertext, 0, ciphertext.length, outputBuf, 0);
      final finalLen = cipher.doFinal(outputBuf, len);

      return outputBuf.sublist(0, len + finalLen);
    } catch (_) {
      return null;
    }
  }
}
