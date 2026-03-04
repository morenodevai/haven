import 'dart:async';
import 'dart:typed_data';

import 'package:haven_app/services/crypto_service.dart';
import 'package:haven_app/services/gateway_service.dart';
import 'package:haven_app/services/win_audio.dart';

/// Captures system audio via WASAPI loopback and relays it encrypted through
/// the gateway as binary 0x05 frames. Receivers play through a separate
/// 48 kHz stereo pipeline so screen share audio doesn't mix with voice.
///
/// Format: 48 kHz stereo 16-bit PCM, 20ms frames (3840 bytes).
class ScreenAudioService {
  final GatewayService _gateway;
  final String _keyBase64;

  WinLoopbackCapture? _capture;
  Timer? _captureTimer;

  /// Receivers: per-user playback instances (48 kHz stereo).
  final Map<String, WinAudioPlayback48kStereo> _playbacks = {};

  bool _receiving = true;

  ScreenAudioService({
    required GatewayService gateway,
    required String keyBase64,
  })  : _gateway = gateway,
        _keyBase64 = keyBase64;

  // ── Sender side ──

  /// Start capturing and sending system audio.
  void startCapture() {
    if (_capture != null) return;

    _capture = WinLoopbackCapture();
    _capture!.start();

    _captureTimer = Timer.periodic(const Duration(milliseconds: 20), (_) {
      _pollAndSend();
    });
  }

  /// Stop capturing system audio.
  void stopCapture() {
    _captureTimer?.cancel();
    _captureTimer = null;
    _capture?.dispose();
    _capture = null;
  }

  // ── Receiver side ──

  /// Handle incoming screen audio from a peer.
  /// Called from the binary message handler when prefix byte is 0x05.
  void handleScreenAudio(String userId, Uint8List encryptedBytes) {
    if (!_receiving) return;

    final pcm = CryptoService.decryptVoiceSyncBytes(_keyBase64, encryptedBytes);
    if (pcm == null || pcm.isEmpty) return;

    // Lazy-create per-user playback
    var playback = _playbacks[userId];
    if (playback == null) {
      playback = WinAudioPlayback48kStereo();
      playback.start();
      _playbacks[userId] = playback;
    }

    // Feed in 3840-byte frames
    int offset = 0;
    while (offset + WinLoopbackCapture.frameBytes <= pcm.length) {
      playback.feed(Uint8List.sublistView(
          pcm, offset, offset + WinLoopbackCapture.frameBytes));
      offset += WinLoopbackCapture.frameBytes;
    }
    // Partial frame at end
    if (offset < pcm.length) {
      playback.feed(Uint8List.sublistView(pcm, offset));
    }
  }

  /// Remove playback for a user who left.
  void removeUser(String userId) {
    _playbacks[userId]?.dispose();
    _playbacks.remove(userId);
  }

  /// Mute/unmute receiving screen audio.
  void setReceiving(bool receiving) {
    _receiving = receiving;
  }

  /// Dispose all resources.
  void dispose() {
    stopCapture();
    for (final p in _playbacks.values) {
      p.dispose();
    }
    _playbacks.clear();
  }

  // ── Internal ──

  static const int _frameBytes = 3840; // 20ms at 48kHz stereo 16-bit

  void _pollAndSend() {
    if (_capture == null || !_capture!.isActive) return;

    final pcm = _capture!.poll();
    if (pcm.isEmpty) return;

    // Split into 20ms frames, encrypt each, send as binary 0x05
    int offset = 0;
    while (offset + _frameBytes <= pcm.length) {
      final frame = Uint8List.sublistView(pcm, offset, offset + _frameBytes);
      _sendFrame(frame);
      offset += _frameBytes;
    }
    if (offset < pcm.length) {
      final frame = Uint8List.sublistView(pcm, offset);
      _sendFrame(frame);
    }
  }

  void _sendFrame(Uint8List pcm) {
    final encrypted = CryptoService.encryptVoiceSyncBytes(_keyBase64, pcm);
    if (encrypted.isEmpty) return;

    // Binary frame: [0x05][encrypted_payload]
    final frame = Uint8List(1 + encrypted.length);
    frame[0] = 0x05;
    frame.setRange(1, frame.length, encrypted);
    _gateway.sendBinary(frame);
  }
}
