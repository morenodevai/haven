/// Haven 2.0 configuration constants.
///
/// All server-related constants match the existing Haven Rust server protocol.
class HavenConstants {
  HavenConstants._();

  /// Default server base URL (HTTP REST API).
  static const String defaultServerUrl = 'http://72.49.142.48:3210';

  /// Default WebSocket gateway URL.
  static String gatewayUrl(String baseUrl) {
    final uri = Uri.parse(baseUrl);
    final wsScheme = uri.scheme == 'https' ? 'wss' : 'ws';
    return '$wsScheme://${uri.host}:${uri.port}/gateway';
  }

  /// Default general channel ID (seeded in server migrations).
  static const String generalChannelId =
      '00000000-0000-0000-0000-000000000001';

  /// Voice channel ID (same as existing Svelte client).
  static const String voiceChannelId =
      '00000000-0000-0000-0000-000000000002';

  /// File-sharing channel ID.
  static const String fileChannelId =
      '00000000-0000-0000-0000-000000000003';

  /// Default shared AES-256 key (base64). All users share this key for MVP.
  /// 32 bytes of 0x41 ('A') = AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=
  static const String defaultChannelKey =
      'QUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUE=';

  /// JWT token refresh threshold — refresh 1 hour before expiry.
  static const Duration tokenRefreshThreshold = Duration(hours: 1);

  /// WebSocket reconnect base delay.
  static const Duration reconnectBaseDelay = Duration(seconds: 1);

  /// WebSocket reconnect max delay.
  static const Duration reconnectMaxDelay = Duration(seconds: 30);

  /// Heartbeat interval (server sends ping every 15s).
  static const Duration heartbeatInterval = Duration(seconds: 15);

  /// Message fetch limit per page.
  static const int messageFetchLimit = 50;

  /// Typing indicator timeout.
  static const Duration typingTimeout = Duration(seconds: 5);

  /// Typing indicator throttle (don't send more than once per 3s).
  static const Duration typingThrottle = Duration(seconds: 3);

  // Binary protocol message types (matching server)
  static const int binaryFileChunk = 0x01;
  static const int binaryFileAck = 0x02;
  static const int binaryFileDone = 0x03;
  static const int binaryVoiceAudio = 0x04;
}
