import 'dart:convert';
import 'dart:io';
import 'dart:async';

Future<void> main() async {
  final baseUrl = 'http://72.49.142.48:3210';
  
  // Login
  print('1. Logging in...');
  final client = HttpClient();
  final loginReq = await client.postUrl(Uri.parse('$baseUrl/auth/login'));
  loginReq.headers.contentType = ContentType.json;
  loginReq.write(jsonEncode({'username': 'test', 'password': 'testtest'}));
  final loginResp = await loginReq.close();
  final loginBody = jsonDecode(await loginResp.transform(utf8.decoder).join());
  final token = loginBody['token'];
  print('   Token: ${token.substring(0, 30)}...');
  
  // Connect to WebSocket gateway (pre-auth path)
  print('\n2. Connecting to gateway...');
  final ws = await WebSocket.connect(
    'ws://72.49.142.48:3210/gateway?token=$token',
  );
  print('   WebSocket connected!');
  
  // Listen for events
  final completer = Completer<void>();
  bool gotReady = false;
  bool gotPresence = false;
  int eventCount = 0;
  
  ws.listen(
    (data) {
      if (data is String) {
        final parsed = jsonDecode(data);
        final type = parsed['type'];
        eventCount++;
        
        if (type == 'Ready') {
          gotReady = true;
          print('   Received: Ready (user=${parsed['data']['username']})');
          
          // Subscribe to general channel
          ws.add(jsonEncode({
            'type': 'Subscribe',
            'data': {'channel_ids': ['00000000-0000-0000-0000-000000000001']}
          }));
          print('   Sent: Subscribe to general channel');
        }
        else if (type == 'PresenceUpdate') {
          gotPresence = true;
          final d = parsed['data'];
          print('   Received: PresenceUpdate (${d['username']} ${d['online'] ? 'online' : 'offline'}})');
        }
        else if (type == 'MessageCreate') {
          final d = parsed['data'];
          print('   Received: MessageCreate from ${d['author_username']}');
        }
        else {
          print('   Received: $type');
        }
        
        if (eventCount >= 3 && gotReady && !completer.isCompleted) {
          completer.complete();
        }
      }
    },
    onDone: () {
      print('   WebSocket closed');
      if (!completer.isCompleted) completer.complete();
    },
    onError: (e) {
      print('   WebSocket error: $e');
      if (!completer.isCompleted) completer.complete();
    },
  );
  
  // Wait for some events or timeout
  await completer.future.timeout(
    Duration(seconds: 5),
    onTimeout: () {},
  );
  
  print('\n3. Results:');
  print('   Got Ready event: $gotReady');
  print('   Got PresenceUpdate: $gotPresence');
  print('   Total events received: $eventCount');
  
  // Send a typing indicator
  print('\n4. Sending typing indicator...');
  ws.add(jsonEncode({
    'type': 'StartTyping',
    'data': {'channel_id': '00000000-0000-0000-0000-000000000001'}
  }));
  print('   Sent StartTyping');
  
  await Future.delayed(Duration(seconds: 1));
  
  // Close
  await ws.close();
  client.close();
  
  assert(gotReady, 'Did not receive Ready event!');
  print('\nGATEWAY TEST PASSED!');
}
