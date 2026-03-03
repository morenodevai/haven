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

  int _inputDeviceId = -1;
  int _outputDeviceId = -1;

  /// Per-user volume: userId → 0.0 (muted) to 2.0 (amplified). Default 1.0.
  final Map<String, double> _userVolumes = {};

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
    _capture!.start(deviceId: _inputDeviceId);

    // Poll mic every 15ms (slightly faster than 20ms frame to avoid missing buffers)
    _captureTimer = Timer.periodic(const Duration(milliseconds: 15), (_) {
      _pollAndSend();
    });
  }

  /// Start audio playback (opens speaker device).
  Future<void> startPlayback() async {
    _playback = WinAudioPlayback();
    _playback!.start(deviceId: _outputDeviceId);
  }

  /// Handle incoming voice audio from another participant.
  void handleAudioData(String userId, String encryptedBase64) {
    if (_deafened || _playback == null || !_playback!.isActive) return;

    final pcm = CryptoService.decryptVoiceSync(_keyBase64, encryptedBase64);
    if (pcm == null || pcm.isEmpty) return;

    // Apply per-user volume scaling
    final volume = _userVolumes[userId] ?? 1.0;
    final scaled = volume == 1.0 ? pcm : _scaleVolume(pcm, volume);

    _playback!.feed(scaled);

    // Remote VAD — update speaking indicator (use original PCM for accuracy)
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

  /// Set volume for a specific user (0.0 = muted, 1.0 = normal, 2.0 = max).
  void setUserVolume(String userId, double volume) {
    _userVolumes[userId] = volume.clamp(0.0, 2.0);
  }

  /// Switch input (microphone) device. Stops and restarts capture.
  void setInputDevice(int deviceId) {
    _inputDeviceId = deviceId;
    if (_capture != null && _capture!.isActive) {
      _captureTimer?.cancel();
      _capture!.stop();
      _capture!.start(deviceId: _inputDeviceId);
      _captureTimer = Timer.periodic(const Duration(milliseconds: 15), (_) {
        _pollAndSend();
      });
    }
  }

  /// Switch output (speaker) device. Stops and restarts playback.
  void setOutputDevice(int deviceId) {
    _outputDeviceId = deviceId;
    if (_playback != null && _playback!.isActive) {
      _playback!.stop();
      _playback!.start(deviceId: _outputDeviceId);
    }
  }

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

  /// Scale 16-bit PCM samples by [factor]. Clamps to Int16 range.
  static Uint8List _scaleVolume(Uint8List pcm, double factor) {
    if (pcm.length < 2) return pcm;
    final result = Uint8List(pcm.length);
    final src = ByteData.view(pcm.buffer, pcm.offsetInBytes, pcm.length);
    final dst = ByteData.view(result.buffer);
    final sampleCount = pcm.length ~/ 2;
    for (int i = 0; i < sampleCount; i++) {
      final sample = src.getInt16(i * 2, Endian.little);
      final scaled = (sample * factor).round().clamp(-32768, 32767);
      dst.setInt16(i * 2, scaled, Endian.little);
    }
    return result;
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
