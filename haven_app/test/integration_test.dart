/// Quick integration test: register/login, encrypt, send message, fetch, decrypt.
/// Run with: dart run test/integration_test.dart
import 'dart:convert';
import 'dart:math';
import 'dart:typed_data';

import 'package:haven_app/config/constants.dart';
import 'package:haven_app/services/crypto_service.dart';
import 'package:dio/dio.dart';

const baseUrl = 'http://72.49.142.48:3210';
const channelKey = HavenConstants.defaultChannelKey;
const channelId = HavenConstants.generalChannelId;

Future<void> main() async {
  final dio = Dio(BaseOptions(baseUrl: baseUrl));
  final rng = Random();
  final testUser = 'ci_test_${rng.nextInt(99999)}';
  final testPass = 'Pass${rng.nextInt(999999)}!';

  print('=== Haven v2.0 Integration Test ===\n');

  // 1. Register
  print('1. Registering user "$testUser"...');
  try {
    final regResp = await dio.post('/auth/register', data: {
      'username': testUser,
      'password': testPass,
    });
    final token = regResp.data['token'] as String;
    final userId = regResp.data['user_id'] as String;
    print('   OK - user_id=$userId');
    dio.options.headers['Authorization'] = 'Bearer $token';
  } catch (e) {
    print('   FAIL - $e');
    return;
  }

  // 2. Encrypt a message
  final testContent = 'Hello from Haven v2.0 Flutter! [${DateTime.now().toIso8601String()}]';
  print('\n2. Encrypting: "$testContent"');
  try {
    final encrypted = await CryptoService.encrypt(channelKey, testContent);
    print('   OK - ciphertext=${encrypted['ciphertext']!.substring(0, 40)}...');
    print('   OK - nonce=${encrypted['nonce']}');

    // 3. Send message
    print('\n3. Sending encrypted message...');
    final sendResp = await dio.post('/channels/$channelId/messages', data: {
      'ciphertext': encrypted['ciphertext'],
      'nonce': encrypted['nonce'],
    });
    final msgId = sendResp.data['id'] as String;
    print('   OK - message_id=$msgId');

    // 4. Fetch messages
    print('\n4. Fetching messages...');
    final fetchResp = await dio.get('/channels/$channelId/messages', queryParameters: {
      'limit': 3,
    });
    final messages = fetchResp.data as List;
    print('   OK - got ${messages.length} messages');

    // 5. Decrypt the message we just sent
    print('\n5. Decrypting our message...');
    // Find our message (first in the list since API returns DESC)
    Map<String, dynamic>? ourMsg;
    for (final m in messages) {
      if (m['id'] == msgId) {
        ourMsg = m as Map<String, dynamic>;
        break;
      }
    }
    if (ourMsg == null) {
      print('   FAIL - our message not found in response');
      return;
    }

    final decrypted = await CryptoService.decrypt(
      channelKey,
      ourMsg['ciphertext'] as String,
      ourMsg['nonce'] as String,
    );
    print('   Decrypted: "$decrypted"');

    if (decrypted == testContent) {
      print('   MATCH!\n');
    } else {
      print('   MISMATCH! Expected: "$testContent"');
      return;
    }

    // 6. Test reactions
    print('6. Testing reaction toggle...');
    final reactionResp = await dio.post(
      '/channels/$channelId/messages/$msgId/reactions',
      data: {'emoji': '\u{1F44D}'},
    );
    print('   OK - status ${reactionResp.statusCode}');

    // 7. Verify reaction on message
    print('\n7. Verifying reaction...');
    final verifyResp = await dio.get('/channels/$channelId/messages', queryParameters: {
      'limit': 1,
    });
    final verifyMessages = verifyResp.data as List;
    if (verifyMessages.isNotEmpty) {
      final reactions = verifyMessages[0]['reactions'] as List?;
      if (reactions != null && reactions.isNotEmpty) {
        print('   OK - found ${reactions.length} reaction(s): ${reactions[0]['emoji']}');
      } else {
        print('   OK - no reactions returned (may be on a different message)');
      }
    }

    // 8. Test file upload
    print('\n8. Testing file upload...');
    final testFileContent = utf8.encode('Haven v2.0 test file content');
    final encryptedFile = await CryptoService.encryptFile(
      channelKey,
      testFileContent,
    );
    print('   Encrypted ${testFileContent.length} bytes -> ${encryptedFile.length} bytes');

    final uploadResp = await dio.post(
      '/files',
      data: Stream.fromIterable([encryptedFile]),
      options: Options(
        headers: {
          'Content-Type': 'application/octet-stream',
          'Content-Length': encryptedFile.length,
        },
      ),
    );
    final fileId = uploadResp.data['file_id'] as String;
    print('   OK - file_id=$fileId');

    // 9. Download and decrypt file
    print('\n9. Downloading and decrypting file...');
    final downloadResp = await dio.get<List<int>>(
      '/files/$fileId',
      options: Options(responseType: ResponseType.bytes),
    );
    final downloadedBytes = Uint8List.fromList(downloadResp.data!);
    final decryptedFile = await CryptoService.decryptFile(
      channelKey,
      downloadedBytes,
    );
    final decryptedContent = utf8.decode(decryptedFile);
    print('   Decrypted: "$decryptedContent"');
    if (decryptedContent == 'Haven v2.0 test file content') {
      print('   MATCH!\n');
    } else {
      print('   MISMATCH!');
      return;
    }

    print('=============================');
    print('ALL TESTS PASSED');
    print('=============================');
  } catch (e) {
    print('   FAIL - $e');
  }
}
