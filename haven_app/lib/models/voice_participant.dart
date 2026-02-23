class VoiceParticipant {
  final String userId;
  final String username;
  final String? sessionId;
  final bool selfMute;
  final bool selfDeaf;
  final bool speaking;

  const VoiceParticipant({
    required this.userId,
    required this.username,
    this.sessionId,
    this.selfMute = false,
    this.selfDeaf = false,
    this.speaking = false,
  });

  VoiceParticipant copyWith({
    String? sessionId,
    bool? selfMute,
    bool? selfDeaf,
    bool? speaking,
  }) {
    return VoiceParticipant(
      userId: userId,
      username: username,
      sessionId: sessionId ?? this.sessionId,
      selfMute: selfMute ?? this.selfMute,
      selfDeaf: selfDeaf ?? this.selfDeaf,
      speaking: speaking ?? this.speaking,
    );
  }

  bool get isConnected => sessionId != null;
}
