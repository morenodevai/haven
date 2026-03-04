import 'dart:typed_data';

import 'package:flutter_riverpod/flutter_riverpod.dart';

import 'package:haven_app/config/constants.dart';
import 'package:haven_app/models/voice_participant.dart';
import 'package:haven_app/providers/audio_settings_provider.dart';
import 'package:haven_app/providers/auth_provider.dart';
import 'package:haven_app/providers/gateway_provider.dart';
import 'package:haven_app/services/screen_audio_service.dart';
import 'package:haven_app/services/voice_service.dart';
import 'package:haven_app/providers/video_provider.dart';
import 'package:haven_app/services/win_audio.dart';

class VoiceState {
  final bool isInVoice;
  final bool selfMute;
  final bool selfDeaf;
  final String? error;
  final Map<String, VoiceParticipant> participants;

  const VoiceState({
    this.isInVoice = false,
    this.selfMute = false,
    this.selfDeaf = false,
    this.error,
    this.participants = const {},
  });

  VoiceState copyWith({
    bool? isInVoice,
    bool? selfMute,
    bool? selfDeaf,
    String? error,
    bool clearError = false,
    Map<String, VoiceParticipant>? participants,
  }) {
    return VoiceState(
      isInVoice: isInVoice ?? this.isInVoice,
      selfMute: selfMute ?? this.selfMute,
      selfDeaf: selfDeaf ?? this.selfDeaf,
      error: clearError ? null : (error ?? this.error),
      participants: participants ?? this.participants,
    );
  }
}

class VoiceNotifier extends StateNotifier<VoiceState> {
  final Ref _ref;
  VoiceService? _voiceService;
  ScreenAudioService? _screenAudioService;
  void Function(Uint8List)? _binaryHandler;

  VoiceNotifier(this._ref) : super(const VoiceState());

  /// Join the voice channel — opens mic + speakers, sends VoiceJoin.
  Future<void> joinVoice() async {
    state = state.copyWith(clearError: true);

    final gateway = _ref.read(gatewayServiceProvider);
    final authState = _ref.read(authProvider);

    try {
      final voiceService = VoiceService(
        gateway: gateway,
        keyBase64: HavenConstants.defaultChannelKey,
        onLocalSpeakingChanged: (speaking) {
          final userId = authState.userId;
          if (userId != null) {
            setSpeaking(userId, speaking);
          }
        },
        onRemoteSpeakingChanged: (userId, speaking) {
          setSpeaking(userId, speaking);
        },
      );

      // Wire audio settings → voice service
      final audioSettings = _ref.read(audioSettingsProvider.notifier);
      audioSettings.onInputDeviceChanged = (id) => voiceService.setInputDevice(id);
      audioSettings.onOutputDeviceChanged = (id) => voiceService.setOutputDevice(id);
      audioSettings.onUserVolumeChanged = (userId, vol) => voiceService.setUserVolume(userId, vol);

      // Apply current audio settings
      final audioState = _ref.read(audioSettingsProvider);
      for (final entry in audioState.userVolumes.entries) {
        voiceService.setUserVolume(entry.key, entry.value);
      }

      // Start playback first — if speakers fail, that's a real error
      await voiceService.startPlayback();

      // Try mic — if unavailable, just auto-mute
      bool noMic = false;
      try {
        await voiceService.startCapture();
      } on AudioException {
        noMic = true;
      }

      _voiceService = voiceService;

      // Screen audio service — receives screen share audio from peers
      _screenAudioService = ScreenAudioService(
        gateway: gateway,
        keyBase64: HavenConstants.defaultChannelKey,
      );

      // Register binary handler for incoming screen audio (0x05)
      _binaryHandler = _handleBinaryMessage;
      gateway.onBinary(_binaryHandler!);

      state = state.copyWith(isInVoice: true, selfMute: noMic, selfDeaf: false);

      gateway.voiceJoin(HavenConstants.voiceChannelId);

      // Initialize WebRTC for video/screen share
      _ref.read(videoProvider.notifier).initWebRTC();
    } on AudioException catch (e) {
      state = state.copyWith(error: e.message);
    } catch (e) {
      state = state.copyWith(error: 'Failed to start voice: $e');
    }
  }

  /// Leave the voice channel — closes audio + video, sends VoiceLeave.
  Future<void> leaveVoice() async {
    final audioSettings = _ref.read(audioSettingsProvider.notifier);
    audioSettings.onInputDeviceChanged = null;
    audioSettings.onOutputDeviceChanged = null;
    audioSettings.onUserVolumeChanged = null;

    await _ref.read(videoProvider.notifier).disposeWebRTC();
    _ref.read(gatewayServiceProvider).voiceLeave();
    await _voiceService?.dispose();
    _voiceService = null;

    // Clean up screen audio
    _screenAudioService?.dispose();
    _screenAudioService = null;
    if (_binaryHandler != null) {
      _ref.read(gatewayServiceProvider).offBinary(_binaryHandler!);
      _binaryHandler = null;
    }

    state = const VoiceState();
  }

  void toggleMute() {
    final newMute = !state.selfMute;
    state = state.copyWith(selfMute: newMute);
    _voiceService?.setMuted(newMute);

    _ref.read(gatewayServiceProvider).voiceStateSet(
          selfMute: newMute,
          selfDeaf: state.selfDeaf,
        );
  }

  void toggleDeaf() {
    final newDeaf = !state.selfDeaf;
    // When deafening, also mute
    final newMute = newDeaf ? true : state.selfMute;

    state = state.copyWith(selfDeaf: newDeaf, selfMute: newMute);
    _voiceService?.setDeafened(newDeaf);
    _voiceService?.setMuted(newMute);

    _ref.read(gatewayServiceProvider).voiceStateSet(
          selfMute: newMute,
          selfDeaf: newDeaf,
        );
  }

  /// Handle VoiceAudioData event from gateway.
  void handleAudioData(Map<String, dynamic> event) {
    final data = event['data'] as Map<String, dynamic>;
    final userId = data['from_user_id'] as String;
    final audioData = data['data'] as String;
    _voiceService?.handleAudioData(userId, audioData);
  }

  /// Handle VoiceStateUpdate event from gateway.
  void handleVoiceStateUpdate(Map<String, dynamic> event) {
    final data = event['data'] as Map<String, dynamic>;
    final userId = data['user_id'] as String;
    final username = data['username'] as String;
    final sessionId = data['session_id'] as String?;
    final selfMute = data['self_mute'] as bool? ?? false;
    final selfDeaf = data['self_deaf'] as bool? ?? false;

    if (sessionId == null) {
      // User left voice
      final updated = Map<String, VoiceParticipant>.from(state.participants);
      updated.remove(userId);
      state = state.copyWith(participants: updated);
    } else {
      // User joined or updated
      final updated = Map<String, VoiceParticipant>.from(state.participants);
      final existing = updated[userId];
      updated[userId] = VoiceParticipant(
        userId: userId,
        username: username,
        sessionId: sessionId,
        selfMute: selfMute,
        selfDeaf: selfDeaf,
        speaking: existing?.speaking ?? false,
      );
      state = state.copyWith(participants: updated);
    }
  }

  void setSpeaking(String userId, bool speaking) {
    final p = state.participants[userId];
    if (p == null || p.speaking == speaking) return;
    final updated = Map<String, VoiceParticipant>.from(state.participants);
    updated[userId] = p.copyWith(speaking: speaking);
    state = state.copyWith(participants: updated);
  }

  /// Start capturing system audio for screen share.
  void startScreenAudioCapture() {
    _screenAudioService?.startCapture();
  }

  /// Stop capturing system audio.
  void stopScreenAudioCapture() {
    _screenAudioService?.stopCapture();
  }

  /// Handle incoming binary WebSocket frames.
  /// Routes 0x05 screen audio to the screen audio service.
  void _handleBinaryMessage(Uint8List data) {
    if (data.length < 18) return; // 1 type + 16 uuid + 1 payload min
    final type = data[0];

    if (type == 0x05) {
      // Screen audio: [0x05][sender_uid(16)][encrypted_payload]
      final senderBytes = data.sublist(1, 17);
      // Convert UUID bytes to string (xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx)
      final hex = senderBytes.map((b) => b.toRadixString(16).padLeft(2, '0')).join();
      final userId = '${hex.substring(0, 8)}-${hex.substring(8, 12)}-'
          '${hex.substring(12, 16)}-${hex.substring(16, 20)}-${hex.substring(20, 32)}';
      final payload = data.sublist(17);
      _screenAudioService?.handleScreenAudio(userId, payload);
    }
  }

  void clear() {
    _screenAudioService?.dispose();
    _screenAudioService = null;
    if (_binaryHandler != null) {
      _ref.read(gatewayServiceProvider).offBinary(_binaryHandler!);
      _binaryHandler = null;
    }
    _voiceService?.dispose();
    _voiceService = null;
    state = const VoiceState();
  }
}

final voiceProvider =
    StateNotifierProvider<VoiceNotifier, VoiceState>((ref) {
  return VoiceNotifier(ref);
});
