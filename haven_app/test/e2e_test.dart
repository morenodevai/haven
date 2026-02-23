import 'dart:convert';
import 'dart:typed_data';
import 'dart:io';
import 'package:pointycastle/export.dart';

Map<String, String> encryptSync(String keyBase64, String plaintext) {
  final key = base64.decode(keyBase64);
  final rng = FortunaRandom();
  final seed = Uint8List(32);
  final dartRandom = DateTime.now().microsecondsSinceEpoch;
  for (int i = 0; i < 32; i++) seed[i] = ((dartRandom >> (i % 8)) ^ (i * 37)) & 0xFF;
  rng.seed(KeyParameter(seed));
  final nonce = Uint8List(12);
  for (int i = 0; i < 12; i++) nonce[i] = rng.nextUint8();

  final plaintextBytes = utf8.encode(plaintext);
  final cipher = GCMBlockCipher(AESEngine())..init(true, AEADParameters(
    KeyParameter(Uint8List.fromList(key)), 128, nonce, Uint8List(0)));
  final buf = Uint8List(cipher.getOutputSize(plaintextBytes.length));
  final len = cipher.processBytes(Uint8List.fromList(plaintextBytes), 0, plaintextBytes.length, buf, 0);
  final finalLen = cipher.doFinal(buf, len);
  final ciphertext = buf.sublist(0, len + finalLen);
  return {'ciphertext': base64.encode(ciphertext), 'nonce': base64.encode(nonce)};
}

String decryptSync(String keyBase64, String ciphertextB64, String nonceB64) {
  final key = base64.decode(keyBase64);
  final ciphertext = base64.decode(ciphertextB64);
  final nonce = base64.decode(nonceB64);
  final cipher = GCMBlockCipher(AESEngine())..init(false, AEADParameters(
    KeyParameter(Uint8List.fromList(key)), 128, Uint8List.fromList(nonce), Uint8List(0)));
  final buf = Uint8List(cipher.getOutputSize(ciphertext.length));
  final len = cipher.processBytes(Uint8List.fromList(ciphertext), 0, ciphertext.length, buf, 0);
  final finalLen = cipher.doFinal(buf, len);
  return utf8.decode(buf.sublist(0, len + finalLen));
}

Future<void> main() async {
  final baseUrl = 'http://72.49.142.48:3210';
  final channelId = '00000000-0000-0000-0000-000000000001';
  final key = 'QUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUE=';
  
  // Step 1: Login
  print('1. Logging in...');
  final client = HttpClient();
  final loginReq = await client.postUrl(Uri.parse('$baseUrl/auth/login'));
  loginReq.headers.contentType = ContentType.json;
  loginReq.write(jsonEncode({'username': 'test', 'password': 'testtest'}));
  final loginResp = await loginReq.close();
  final loginBody = jsonDecode(await loginResp.transform(utf8.decoder).join());
  final token = loginBody['token'];
  print('   Login OK - user_id: ${loginBody['user_id']}');
  
  // Step 2: Fetch existing messages and try to decrypt them
  print('\n2. Fetching messages...');
  final msgReq = await client.getUrl(Uri.parse('$baseUrl/channels/$channelId/messages?limit=5'));
  msgReq.headers.add('Authorization', 'Bearer $token');
  final msgResp = await msgReq.close();
  final messages = jsonDecode(await msgResp.transform(utf8.decoder).join()) as List;
  print('   Got ${messages.length} messages');
  
  for (final msg in messages.take(3)) {
    try {
      final decrypted = decryptSync(key, msg['ciphertext'], msg['nonce']);
      // Truncate long messages for display
      final display = decrypted.length > 80 ? '${decrypted.substring(0, 80)}...' : decrypted;
      print('   [${msg['author_username']}]: $display');
    } catch (e) {
      print('   [${msg['author_username']}]: [decrypt failed: $e]');
    }
  }
  
  // Step 3: Send a test message
  print('\n3. Sending encrypted message...');
  final encrypted = encryptSync(key, 'Hello from Haven 2.0! (Dart crypto test)');
  final sendReq = await client.postUrl(Uri.parse('$baseUrl/channels/$channelId/messages'));
  sendReq.headers.contentType = ContentType.json;
  sendReq.headers.add('Authorization', 'Bearer $token');
  sendReq.write(jsonEncode({'ciphertext': encrypted['ciphertext'], 'nonce': encrypted['nonce']}));
  final sendResp = await sendReq.close();
  final sendBody = jsonDecode(await sendResp.transform(utf8.decoder).join());
  print('   Sent! Message ID: ${sendBody['id']}');
  
  // Step 4: Fetch it back and decrypt to verify
  print('\n4. Verifying sent message...');
  final verifyReq = await client.getUrl(Uri.parse('$baseUrl/channels/$channelId/messages?limit=1'));
  verifyReq.headers.add('Authorization', 'Bearer $token');
  final verifyResp = await verifyReq.close();
  final verifyBody = jsonDecode(await verifyResp.transform(utf8.decoder).join()) as List;
  final latest = verifyBody.first;
  final decrypted = decryptSync(key, latest['ciphertext'], latest['nonce']);
  print('   Decrypted: $decrypted');
  assert(decrypted == 'Hello from Haven 2.0! (Dart crypto test)', 'Roundtrip failed!');
  
  print('\nE2E TEST PASSED - Messages encrypt, send, receive, and decrypt correctly!');
  client.close();
}
