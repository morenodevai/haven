class VoiceParticipant {
  final String userId;
  final String username;
  final String? sessionId;
  final bool selfMute;
  final bool selfDeaf;
  final bool speaking;
  final bool cameraOn;
  final bool screenSharing;

  const VoiceParticipant({
    required this.userId,
    required this.username,
    this.sessionId,
    this.selfMute = false,
    this.selfDeaf = false,
    this.speaking = false,
    this.cameraOn = false,
    this.screenSharing = false,
  });

  VoiceParticipant copyWith({
    String? sessionId,
    bool? selfMute,
    bool? selfDeaf,
    bool? speaking,
    bool? cameraOn,
    bool? screenSharing,
  }) {
    return VoiceParticipant(
      userId: userId,
      username: username,
      sessionId: sessionId ?? this.sessionId,
      selfMute: selfMute ?? this.selfMute,
      selfDeaf: selfDeaf ?? this.selfDeaf,
      speaking: speaking ?? this.speaking,
      cameraOn: cameraOn ?? this.cameraOn,
      screenSharing: screenSharing ?? this.screenSharing,
    );
  }

  bool get isConnected => sessionId != null;
}
