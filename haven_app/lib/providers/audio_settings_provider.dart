import 'package:flutter_riverpod/flutter_riverpod.dart';

import 'package:haven_app/services/win_audio.dart';

class AudioSettingsState {
  final int inputDeviceId; // -1 = system default
  final int outputDeviceId; // -1 = system default
  final Map<String, double> userVolumes; // userId → 0.0-2.0
  final double screenAudioVolume; // 0.0-2.0
  final bool screenAudioMuted;

  const AudioSettingsState({
    this.inputDeviceId = -1,
    this.outputDeviceId = -1,
    this.userVolumes = const {},
    this.screenAudioVolume = 1.0,
    this.screenAudioMuted = false,
  });

  AudioSettingsState copyWith({
    int? inputDeviceId,
    int? outputDeviceId,
    Map<String, double>? userVolumes,
    double? screenAudioVolume,
    bool? screenAudioMuted,
  }) {
    return AudioSettingsState(
      inputDeviceId: inputDeviceId ?? this.inputDeviceId,
      outputDeviceId: outputDeviceId ?? this.outputDeviceId,
      userVolumes: userVolumes ?? this.userVolumes,
      screenAudioVolume: screenAudioVolume ?? this.screenAudioVolume,
      screenAudioMuted: screenAudioMuted ?? this.screenAudioMuted,
    );
  }
}

class AudioSettingsNotifier extends StateNotifier<AudioSettingsState> {
  AudioSettingsNotifier() : super(const AudioSettingsState());

  /// Callback set by VoiceProvider when voice is active.
  void Function(int deviceId)? onInputDeviceChanged;
  void Function(int deviceId)? onOutputDeviceChanged;
  void Function(String userId, double volume)? onUserVolumeChanged;
  void Function(double volume)? onScreenAudioVolumeChanged;

  void setInputDevice(int deviceId) {
    state = state.copyWith(inputDeviceId: deviceId);
    onInputDeviceChanged?.call(deviceId);
  }

  void setOutputDevice(int deviceId) {
    state = state.copyWith(outputDeviceId: deviceId);
    onOutputDeviceChanged?.call(deviceId);
  }

  void setUserVolume(String userId, double volume) {
    final v = volume.clamp(0.0, 2.0);
    final updated = Map<String, double>.from(state.userVolumes);
    updated[userId] = v;
    state = state.copyWith(userVolumes: updated);
    onUserVolumeChanged?.call(userId, v);
  }

  double getUserVolume(String userId) {
    return state.userVolumes[userId] ?? 1.0;
  }

  void setScreenAudioVolume(double volume) {
    final v = volume.clamp(0.0, 2.0);
    state = state.copyWith(screenAudioVolume: v);
    onScreenAudioVolumeChanged?.call(v);
  }

  void toggleScreenAudioMute() {
    final muted = !state.screenAudioMuted;
    state = state.copyWith(screenAudioMuted: muted);
    onScreenAudioVolumeChanged?.call(muted ? 0.0 : state.screenAudioVolume);
  }

  List<AudioDevice> get inputDevices => getInputDevices();
  List<AudioDevice> get outputDevices => getOutputDevices();
}

final audioSettingsProvider =
    StateNotifierProvider<AudioSettingsNotifier, AudioSettingsState>((ref) {
  return AudioSettingsNotifier();
});
