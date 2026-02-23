class ReactionGroup {
  final String emoji;
  final int count;
  final List<String> userIds;

  const ReactionGroup({
    required this.emoji,
    required this.count,
    required this.userIds,
  });

  factory ReactionGroup.fromJson(Map<String, dynamic> json) {
    return ReactionGroup(
      emoji: json['emoji'] as String,
      count: json['count'] as int,
      userIds: (json['user_ids'] as List<dynamic>)
          .map((e) => e as String)
          .toList(),
    );
  }
}

class Message {
  final String id;
  final String channelId;
  final String authorId;
  final String authorUsername;
  final String content;
  final String timestamp;
  final List<ReactionGroup> reactions;
  final String? imageData;
  final String? imageName;
  final String? videoUrl;
  final String? videoName;
  final String? fileId;
  final String? fileName;
  final int? fileSize;

  const Message({
    required this.id,
    required this.channelId,
    required this.authorId,
    required this.authorUsername,
    required this.content,
    required this.timestamp,
    this.reactions = const [],
    this.imageData,
    this.imageName,
    this.videoUrl,
    this.videoName,
    this.fileId,
    this.fileName,
    this.fileSize,
  });

  bool get isFileMessage => fileId != null;

  Message copyWith({
    String? content,
    List<ReactionGroup>? reactions,
    String? imageData,
    String? imageName,
    String? videoUrl,
    String? videoName,
    String? fileId,
    String? fileName,
    int? fileSize,
  }) {
    return Message(
      id: id,
      channelId: channelId,
      authorId: authorId,
      authorUsername: authorUsername,
      content: content ?? this.content,
      timestamp: timestamp,
      reactions: reactions ?? this.reactions,
      imageData: imageData ?? this.imageData,
      imageName: imageName ?? this.imageName,
      videoUrl: videoUrl ?? this.videoUrl,
      videoName: videoName ?? this.videoName,
      fileId: fileId ?? this.fileId,
      fileName: fileName ?? this.fileName,
      fileSize: fileSize ?? this.fileSize,
    );
  }
}
