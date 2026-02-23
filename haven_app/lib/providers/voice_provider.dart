import 'package:flutter_riverpod/flutter_riverpod.dart';

import 'package:haven_app/config/constants.dart';
import 'package:haven_app/models/voice_participant.dart';
import 'package:haven_app/providers/auth_provider.dart';
import 'package:haven_app/providers/gateway_provider.dart';
import 'package:haven_app/services/voice_service.dart';
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

      await voiceService.startCapture();
      await voiceService.startPlayback();

      _voiceService = voiceService;
      state = state.copyWith(isInVoice: true, selfMute: false, selfDeaf: false);

      gateway.voiceJoin(HavenConstants.voiceChannelId);
    } on AudioException catch (e) {
      state = state.copyWith(error: e.message);
    } catch (e) {
      state = state.copyWith(error: 'Failed to start voice: $e');
    }
  }

  /// Leave the voice channel — closes audio, sends VoiceLeave.
  Future<void> leaveVoice() async {
    _ref.read(gatewayServiceProvider).voiceLeave();
    await _voiceService?.dispose();
    _voiceService = null;
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

  void clear() {
    _voiceService?.dispose();
    _voiceService = null;
    state = const VoiceState();
  }
}

final voiceProvider =
    StateNotifierProvider<VoiceNotifier, VoiceState>((ref) {
  return VoiceNotifier(ref);
});
