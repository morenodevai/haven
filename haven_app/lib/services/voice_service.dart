import 'dart:async';
import 'dart:math';
import 'dart:typed_data';

import 'package:haven_app/services/crypto_service.dart';
import 'package:haven_app/services/gateway_service.dart';
import 'package:haven_app/services/win_audio.dart';

/// Orchestrates the voice audio pipeline:
///   Mic → encrypt → gateway → (server relay) → decrypt → speakers
///
/// Audio format: 16 kHz mono 16-bit PCM (matching existing Svelte client).
/// Encryption: AES-256-GCM, voice format = base64(IV + ciphertext + tag).
class VoiceService {
  final GatewayService _gateway;
  final String _keyBase64;
  final void Function(bool speaking)? onLocalSpeakingChanged;
  final void Function(String userId, bool speaking)? onRemoteSpeakingChanged;

  WinAudioCapture? _capture;
  WinAudioPlayback? _playback;
  Timer? _captureTimer;

  bool _muted = false;
  bool _deafened = false;

  // Local VAD state (with hysteresis to prevent flickering)
  bool _localSpeaking = false;
  static const double _speakThreshold = 0.015;
  static const double _silenceThreshold = 0.008;

  VoiceService({
    required GatewayService gateway,
    required String keyBase64,
    this.onLocalSpeakingChanged,
    this.onRemoteSpeakingChanged,
  })  : _gateway = gateway,
        _keyBase64 = keyBase64;

  /// Start microphone capture. Polls every 20ms and sends encrypted audio.
  Future<void> startCapture() async {
    _capture = WinAudioCapture();
    _capture!.start();

    // Poll mic every 15ms (slightly faster than 20ms frame to avoid missing buffers)
    _captureTimer = Timer.periodic(const Duration(milliseconds: 15), (_) {
      _pollAndSend();
    });
  }

  /// Start audio playback (opens speaker device).
  Future<void> startPlayback() async {
    _playback = WinAudioPlayback();
    _playback!.start();
  }

  /// Handle incoming voice audio from another participant.
  void handleAudioData(String userId, String encryptedBase64) {
    if (_deafened || _playback == null || !_playback!.isActive) return;

    final pcm = CryptoService.decryptVoiceSync(_keyBase64, encryptedBase64);
    if (pcm == null || pcm.isEmpty) return;

    _playback!.feed(pcm);

    // Remote VAD — update speaking indicator
    final speaking = _computeRms(pcm) > 0.01;
    onRemoteSpeakingChanged?.call(userId, speaking);
  }

  /// Toggle mute (stops sending audio, mic stays open for VAD).
  void setMuted(bool muted) {
    _muted = muted;
    if (muted) {
      _localSpeaking = false;
      onLocalSpeakingChanged?.call(false);
    }
  }

  /// Toggle deafen (stops receiving audio).
  void setDeafened(bool deafened) => _deafened = deafened;

  /// Stop all audio and release resources.
  Future<void> dispose() async {
    _captureTimer?.cancel();
    _captureTimer = null;

    _capture?.dispose();
    _capture = null;

    _playback?.dispose();
    _playback = null;
  }

  // ── Internal ──

  void _pollAndSend() {
    if (_capture == null || !_capture!.isActive) return;

    final pcm = _capture!.poll();
    if (pcm.isEmpty) return;

    // Local VAD (always runs, even when muted, so speaking indicator works)
    _updateLocalVad(pcm);

    if (_muted) return;

    // Encrypt and send
    final encrypted = CryptoService.encryptVoiceSync(_keyBase64, pcm);
    if (encrypted.isNotEmpty) {
      _gateway.voiceData(encrypted);
    }
  }

  void _updateLocalVad(Uint8List pcm) {
    final rms = _computeRms(pcm);

    if (_localSpeaking) {
      if (rms < _silenceThreshold) {
        _localSpeaking = false;
        onLocalSpeakingChanged?.call(false);
      }
    } else {
      if (rms > _speakThreshold) {
        _localSpeaking = true;
        if (!_muted) {
          onLocalSpeakingChanged?.call(true);
        }
      }
    }
  }

  /// Compute RMS of 16-bit PCM audio samples.
  static double _computeRms(Uint8List pcm) {
    if (pcm.length < 2) return 0;

    // Interpret bytes as Int16 samples (little-endian)
    final byteData = ByteData.view(pcm.buffer, pcm.offsetInBytes, pcm.length);
    final sampleCount = pcm.length ~/ 2;
    double sumSquares = 0;

    for (int i = 0; i < sampleCount; i++) {
      final sample = byteData.getInt16(i * 2, Endian.little);
      final normalized = sample / (sample < 0 ? 32768 : 32767);
      sumSquares += normalized * normalized;
    }

    return sqrt(sumSquares / sampleCount);
  }
}
