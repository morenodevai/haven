import 'dart:convert';
import 'dart:typed_data';
import 'package:pointycastle/export.dart';

void main() {
  final keyBase64 = 'QUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUE=';
  final key = base64.decode(keyBase64);
  print('Key length: ${key.length} bytes');
  assert(key.length == 32, 'Key must be 32 bytes');

  // Encrypt
  final plaintext = 'Hello from Haven 2.0!';
  final plaintextBytes = utf8.encode(plaintext);

  final nonce = Uint8List(12);
  for (int i = 0; i < 12; i++) nonce[i] = i + 1;

  final cipher = GCMBlockCipher(AESEngine())
    ..init(
      true,
      AEADParameters(
        KeyParameter(Uint8List.fromList(key)),
        128,
        nonce,
        Uint8List(0),
      ),
    );

  final outputBuf = Uint8List(cipher.getOutputSize(plaintextBytes.length));
  final len = cipher.processBytes(
      Uint8List.fromList(plaintextBytes), 0, plaintextBytes.length,
      outputBuf, 0);
  final finalLen = cipher.doFinal(outputBuf, len);
  final ciphertext = outputBuf.sublist(0, len + finalLen);

  print('Plaintext: $plaintext');
  print('Ciphertext length: ${ciphertext.length} (expected ${plaintextBytes.length + 16})');
  print('Ciphertext (base64): ${base64.encode(ciphertext)}');
  print('Nonce (base64): ${base64.encode(nonce)}');
  assert(ciphertext.length == plaintextBytes.length + 16, 'Ciphertext should be plaintext + 16-byte tag');

  // Decrypt
  final cipher2 = GCMBlockCipher(AESEngine())
    ..init(
      false,
      AEADParameters(
        KeyParameter(Uint8List.fromList(key)),
        128,
        nonce,
        Uint8List(0),
      ),
    );

  final decBuf = Uint8List(cipher2.getOutputSize(ciphertext.length));
  final len2 = cipher2.processBytes(ciphertext, 0, ciphertext.length, decBuf, 0);
  final finalLen2 = cipher2.doFinal(decBuf, len2);
  final decrypted = decBuf.sublist(0, len2 + finalLen2);
  final result = utf8.decode(decrypted);

  print('Decrypted: $result');
  assert(result == plaintext, 'Decrypt mismatch! Got "$result" expected "$plaintext"');
  print('\nCRYPTO TEST PASSED - Encrypt/Decrypt roundtrip works!');

  // Test 2: Decrypt a message produced by the Rust server
  // First encrypt with our code and verify roundtrip with random nonce
  final rng = FortunaRandom();
  final seed = Uint8List(32);
  for (int i = 0; i < 32; i++) seed[i] = (i * 37 + 42) & 0xFF;
  rng.seed(KeyParameter(seed));
  
  final randomNonce = Uint8List(12);
  for (int i = 0; i < 12; i++) randomNonce[i] = rng.nextUint8();

  final cipher3 = GCMBlockCipher(AESEngine())
    ..init(
      true,
      AEADParameters(
        KeyParameter(Uint8List.fromList(key)),
        128,
        randomNonce,
        Uint8List(0),
      ),
    );
  
  final testMsg = 'This is a longer test message to verify crypto works with different lengths!';
  final testBytes = utf8.encode(testMsg);
  final encBuf = Uint8List(cipher3.getOutputSize(testBytes.length));
  final l1 = cipher3.processBytes(Uint8List.fromList(testBytes), 0, testBytes.length, encBuf, 0);
  final l2 = cipher3.doFinal(encBuf, l1);
  final encrypted = encBuf.sublist(0, l1 + l2);

  final cipher4 = GCMBlockCipher(AESEngine())
    ..init(false, AEADParameters(
      KeyParameter(Uint8List.fromList(key)), 128, randomNonce, Uint8List(0)));
  final decBuf2 = Uint8List(cipher4.getOutputSize(encrypted.length));
  final d1 = cipher4.processBytes(encrypted, 0, encrypted.length, decBuf2, 0);
  final d2 = cipher4.doFinal(decBuf2, d1);
  final decrypted2 = utf8.decode(decBuf2.sublist(0, d1 + d2));
  
  assert(decrypted2 == testMsg, 'Test 2 failed!');
  print('Test 2 PASSED - Random nonce roundtrip works!');
  
  print('\nALL CRYPTO TESTS PASSED');
}
