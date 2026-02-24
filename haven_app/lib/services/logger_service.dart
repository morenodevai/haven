import 'dart:convert';

import 'package:web_socket_channel/web_socket_channel.dart';

/// Centralized logging for Haven 2.5.
///
/// Sends all log messages to the server via WebSocket for centralized
/// debugging. Also prints to console for local dev use.
///
/// Usage:
///   Log.d('Gateway', 'Connected to $url');
///   Log.e('Auth', 'Login failed', error: e, stack: st);
class Log {
  Log._();

  static WebSocketChannel? _channel;
  static bool _initialized = false;

  /// Initialize logging. Call once at startup.
  static void init() {
    if (_initialized) return;
    _initialized = true;
  }

  /// Set the WebSocket channel for sending logs to the server.
  /// Call when gateway connects; pass null when it disconnects.
  static void setChannel(WebSocketChannel? channel) {
    _channel = channel;
  }

  /// Debug — verbose details for tracing execution flow.
  static void d(String tag, String message) => _write('DEBUG', tag, message);

  /// Info — key lifecycle events and state changes.
  static void i(String tag, String message) => _write('INFO', tag, message);

  /// Warning — unexpected but recoverable situations.
  static void w(String tag, String message, {Object? error}) {
    final msg = error != null ? '$message — $error' : message;
    _write('WARN', tag, msg);
  }

  /// Error — failures that affect functionality.
  static void e(String tag, String message,
      {Object? error, StackTrace? stack}) {
    final parts = StringBuffer(message);
    if (error != null) parts.write(' — $error');
    _write('ERROR', tag, parts.toString());
    if (stack != null) {
      _write('ERROR', tag, 'Stack:\n$stack');
    }
  }

  static void _write(String level, String tag, String message) {
    final now = DateTime.now();
    final ts =
        '${now.hour.toString().padLeft(2, '0')}:'
        '${now.minute.toString().padLeft(2, '0')}:'
        '${now.second.toString().padLeft(2, '0')}.'
        '${now.millisecond.toString().padLeft(3, '0')}';
    final line = '[$ts] $level/$tag: $message';

    // Console (always)
    // ignore: avoid_print
    print(line);

    // Server (when connected)
    _sendToServer(level, tag, message);
  }

  static void _sendToServer(String level, String tag, String message) {
    final ch = _channel;
    if (ch == null) return;

    // Don't send Gateway logs to avoid feedback loops
    if (tag == 'Gateway') return;

    try {
      ch.sink.add(jsonEncode({
        'type': 'LogSend',
        'data': {
          'level': level,
          'tag': tag,
          'message': message,
        },
      }));
    } catch (_) {
      // Silently drop — can't log a logging failure
    }
  }

  /// No-op kept for API compatibility.
  static Future<void> close() async {}
}
