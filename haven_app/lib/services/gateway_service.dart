import 'dart:async';
import 'dart:convert';
import 'dart:math';
import 'dart:typed_data';

import 'package:web_socket_channel/web_socket_channel.dart';

import 'package:haven_app/config/constants.dart';

/// Callback types for gateway events.
typedef EventHandler = void Function(Map<String, dynamic> data);
typedef BinaryHandler = void Function(Uint8List data);
typedef ConnectionHandler = void Function();

/// WebSocket gateway client for the Haven server.
///
/// Handles:
/// - Connection with JWT auth via query param (pre-auth path)
/// - JSON event dispatch (tagged {"type": "...", "data": {...}})
/// - Binary frame dispatch (file chunks, voice audio)
/// - Auto-reconnect with exponential backoff + jitter
/// - Send queue for messages during reconnection
class GatewayService {
  final String Function() _getToken;
  final String Function() _getBaseUrl;

  WebSocketChannel? _channel;
  StreamSubscription? _subscription;
  bool _closed = false;
  int _reconnectAttempts = 0;
  Timer? _reconnectTimer;
  final List<String> _sendQueue = [];

  final Map<String, List<EventHandler>> _handlers = {};
  final List<BinaryHandler> _binaryHandlers = [];
  final List<ConnectionHandler> _connectHandlers = [];
  final List<ConnectionHandler> _disconnectHandlers = [];

  bool _isConnected = false;

  GatewayService({
    required String Function() getToken,
    required String Function() getBaseUrl,
  })  : _getToken = getToken,
        _getBaseUrl = getBaseUrl;

  bool get isConnected => _isConnected;

  /// Connect to the gateway.
  void connect() {
    _closed = false;
    _doConnect();
  }

  void _doConnect() {
    if (_closed) return;

    try {
      final baseUrl = _getBaseUrl();
      final token = _getToken();
      final wsUrl = HavenConstants.gatewayUrl(baseUrl);
      final uri = Uri.parse('$wsUrl?token=$token');

      _channel = WebSocketChannel.connect(uri);

      _subscription = _channel!.stream.listen(
        (data) {
          if (data is String) {
            _handleTextMessage(data);
          } else if (data is List<int>) {
            _handleBinaryMessage(Uint8List.fromList(data));
          }
        },
        onDone: () {
          _isConnected = false;
          for (final handler in _disconnectHandlers) {
            handler();
          }
          if (!_closed) {
            _scheduleReconnect();
          }
        },
        onError: (error) {
          // onDone will fire after — reconnect handled there
        },
      );

      // Mark connected once the WebSocket is ready
      // The server sends Ready event immediately after upgrade
      _isConnected = true;
      _reconnectAttempts = 0;
      _flushQueue();

      for (final handler in _connectHandlers) {
        handler();
      }
    } catch (e) {
      _isConnected = false;
      if (!_closed) {
        _scheduleReconnect();
      }
    }
  }

  /// Disconnect from the gateway.
  void disconnect() {
    _closed = true;
    _reconnectTimer?.cancel();
    _reconnectTimer = null;
    _subscription?.cancel();
    _subscription = null;
    _channel?.sink.close();
    _channel = null;
    _sendQueue.clear();
    _isConnected = false;
  }

  /// Send a JSON command to the server.
  void send(Map<String, dynamic> data) {
    final json = jsonEncode(data);
    if (_isConnected && _channel != null) {
      _channel!.sink.add(json);
    } else {
      _sendQueue.add(json);
    }
  }

  /// Send raw binary data.
  void sendBinary(Uint8List data) {
    if (_isConnected && _channel != null) {
      _channel!.sink.add(data);
    }
    // Binary data not queued — file transfer handles retries
  }

  // -- Event registration --

  void on(String event, EventHandler handler) {
    _handlers.putIfAbsent(event, () => []).add(handler);
  }

  void off(String event, EventHandler handler) {
    _handlers[event]?.remove(handler);
  }

  void onBinary(BinaryHandler handler) {
    _binaryHandlers.add(handler);
  }

  void offBinary(BinaryHandler handler) {
    _binaryHandlers.remove(handler);
  }

  void onConnect(ConnectionHandler handler) {
    _connectHandlers.add(handler);
  }

  void onDisconnect(ConnectionHandler handler) {
    _disconnectHandlers.add(handler);
  }

  // -- Convenience commands --

  void subscribe(List<String> channelIds) {
    send({'type': 'Subscribe', 'data': {'channel_ids': channelIds}});
  }

  void startTyping(String channelId) {
    send({'type': 'StartTyping', 'data': {'channel_id': channelId}});
  }

  void voiceJoin(String channelId) {
    send({'type': 'VoiceJoin', 'data': {'channel_id': channelId}});
  }

  void voiceLeave() {
    send({'type': 'VoiceLeave', 'data': null});
  }

  void voiceStateSet({required bool selfMute, required bool selfDeaf}) {
    send({
      'type': 'VoiceStateSet',
      'data': {'self_mute': selfMute, 'self_deaf': selfDeaf},
    });
  }

  void voiceSignalSend(String targetUserId, Map<String, dynamic> signal) {
    send({
      'type': 'VoiceSignalSend',
      'data': {'target_user_id': targetUserId, 'signal': signal},
    });
  }

  void voiceData(String data) {
    send({'type': 'VoiceData', 'data': {'data': data}});
  }

  void fileOfferSend({
    required String targetUserId,
    required String transferId,
    required String filename,
    required int size,
  }) {
    send({
      'type': 'FileOfferSend',
      'data': {
        'target_user_id': targetUserId,
        'transfer_id': transferId,
        'filename': filename,
        'size': size,
      },
    });
  }

  void fileAcceptSend(String targetUserId, String transferId) {
    send({
      'type': 'FileAcceptSend',
      'data': {
        'target_user_id': targetUserId,
        'transfer_id': transferId,
      },
    });
  }

  void fileRejectSend(String targetUserId, String transferId) {
    send({
      'type': 'FileRejectSend',
      'data': {
        'target_user_id': targetUserId,
        'transfer_id': transferId,
      },
    });
  }

  // -- Internal --

  void _handleTextMessage(String text) {
    try {
      final parsed = jsonDecode(text) as Map<String, dynamic>;
      final type = parsed['type'] as String?;
      if (type != null) {
        final handlers = _handlers[type];
        if (handlers != null) {
          for (final handler in handlers) {
            handler(parsed);
          }
        }
      }
    } catch (_) {
      // Malformed message — ignore
    }
  }

  void _handleBinaryMessage(Uint8List data) {
    for (final handler in _binaryHandlers) {
      handler(data);
    }
  }

  void _flushQueue() {
    final queue = List<String>.from(_sendQueue);
    _sendQueue.clear();
    for (final msg in queue) {
      if (_isConnected && _channel != null) {
        _channel!.sink.add(msg);
      }
    }
  }

  void _scheduleReconnect() {
    final baseMs = HavenConstants.reconnectBaseDelay.inMilliseconds;
    final maxMs = HavenConstants.reconnectMaxDelay.inMilliseconds;
    final delay = min(baseMs * pow(2, _reconnectAttempts).toInt(), maxMs);
    final jitter = (delay * 0.5 * Random().nextDouble()).toInt();
    _reconnectAttempts++;

    _reconnectTimer?.cancel();
    _reconnectTimer = Timer(
      Duration(milliseconds: delay + jitter),
      _doConnect,
    );
  }
}
