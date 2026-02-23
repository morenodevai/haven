import 'dart:convert';
import 'dart:io';

import 'package:flutter_riverpod/flutter_riverpod.dart';

import 'package:haven_app/config/constants.dart';
import 'package:haven_app/models/message.dart';
import 'package:haven_app/providers/auth_provider.dart';
import 'package:haven_app/services/api_service.dart';
import 'package:haven_app/services/crypto_service.dart';
import 'package:haven_app/utils/formatters.dart';

class MessageState {
  final List<Message> messages;
  final bool isLoading;
  final bool hasMore;
  final String? error;

  const MessageState({
    this.messages = const [],
    this.isLoading = false,
    this.hasMore = true,
    this.error,
  });

  MessageState copyWith({
    List<Message>? messages,
    bool? isLoading,
    bool? hasMore,
    String? error,
  }) {
    return MessageState(
      messages: messages ?? this.messages,
      isLoading: isLoading ?? this.isLoading,
      hasMore: hasMore ?? this.hasMore,
      error: error,
    );
  }
}

class MessageNotifier extends StateNotifier<MessageState> {
  final ApiService _api;
  final String channelKey;

  MessageNotifier(this._api, {required this.channelKey})
      : super(const MessageState());

  /// Load initial messages for the general channel.
  Future<void> loadMessages() async {
    if (state.isLoading) return;
    state = state.copyWith(isLoading: true, error: null);

    try {
      final raw = await _api.getMessages(
        HavenConstants.generalChannelId,
        limit: HavenConstants.messageFetchLimit,
      );
      final messages = await _decryptMessages(raw);
      // API returns DESC order, reverse for display (oldest first)
      state = MessageState(
        messages: messages.reversed.toList(),
        hasMore: raw.length >= HavenConstants.messageFetchLimit,
      );
    } catch (e) {
      state = state.copyWith(isLoading: false, error: 'Failed to load messages');
    }
  }

  /// Load older messages (cursor-based pagination).
  Future<void> loadMoreMessages() async {
    if (state.isLoading || !state.hasMore || state.messages.isEmpty) return;
    state = state.copyWith(isLoading: true);

    try {
      final oldest = state.messages.first;
      final raw = await _api.getMessages(
        HavenConstants.generalChannelId,
        limit: HavenConstants.messageFetchLimit,
        before: oldest.timestamp,
      );
      final older = await _decryptMessages(raw);
      state = state.copyWith(
        messages: [...older.reversed, ...state.messages],
        isLoading: false,
        hasMore: raw.length >= HavenConstants.messageFetchLimit,
      );
    } catch (e) {
      state = state.copyWith(isLoading: false);
    }
  }

  /// Send an encrypted message.
  Future<void> sendMessage(String content) async {
    final encrypted = await CryptoService.encrypt(channelKey, content);
    await _api.sendMessage(
      HavenConstants.generalChannelId,
      encrypted['ciphertext']!,
      encrypted['nonce']!,
    );
  }

  /// Send an image message (inline encrypted).
  Future<void> sendImageMessage({
    required String name,
    required String mime,
    required String base64Data,
  }) async {
    final envelope = jsonEncode({
      'type': 'image',
      'name': name,
      'mime': mime,
      'data': base64Data,
    });
    final encrypted = await CryptoService.encrypt(channelKey, envelope);
    await _api.sendMessage(
      HavenConstants.generalChannelId,
      encrypted['ciphertext']!,
      encrypted['nonce']!,
    );
  }

  /// Upload an encrypted file and send a file message to the channel.
  Future<void> sendFileMessage(String filePath) async {
    final file = File(filePath);
    final fileName = file.uri.pathSegments.last;
    final fileBytes = await file.readAsBytes();
    final fileSize = fileBytes.length;

    // Encrypt the file
    final encryptedBytes =
        await CryptoService.encryptFile(channelKey, fileBytes);

    // Upload encrypted bytes to server
    final result = await _api.uploadFile(encryptedBytes);
    final fileId = result['file_id'] as String;

    // Send a message with the file envelope
    final envelope = jsonEncode({
      'type': 'file',
      'file_id': fileId,
      'name': fileName,
      'size': fileSize,
    });
    final encrypted = await CryptoService.encrypt(channelKey, envelope);
    await _api.sendMessage(
      HavenConstants.generalChannelId,
      encrypted['ciphertext']!,
      encrypted['nonce']!,
    );
  }

  /// Download and decrypt a file from the server.
  Future<void> downloadAndSaveFile(
      String fileId, String savePath) async {
    final encryptedBytes = await _api.downloadFile(fileId);
    final decryptedBytes =
        await CryptoService.decryptFile(channelKey, encryptedBytes);
    await File(savePath).writeAsBytes(decryptedBytes);
  }

  /// Handle an incoming MessageCreate gateway event.
  Future<void> handleIncomingMessage(Map<String, dynamic> event) async {
    final data = event['data'] as Map<String, dynamic>;
    try {
      final plaintext = await CryptoService.decrypt(
        channelKey,
        data['ciphertext'] as String,
        data['nonce'] as String,
      );
      final parsed = _parseContent(plaintext);
      final message = Message(
        id: data['id'] as String,
        channelId: data['channel_id'] as String,
        authorId: data['author_id'] as String,
        authorUsername: data['author_username'] as String,
        content: parsed['content'] as String,
        timestamp: data['timestamp'] as String,
        imageData: parsed['imageData'] as String?,
        imageName: parsed['imageName'] as String?,
        fileId: parsed['fileId'] as String?,
        fileName: parsed['fileName'] as String?,
        fileSize: parsed['fileSize'] as int?,
      );
      state = state.copyWith(messages: [...state.messages, message]);
    } catch (_) {
      final message = Message(
        id: data['id'] as String,
        channelId: data['channel_id'] as String,
        authorId: data['author_id'] as String,
        authorUsername: data['author_username'] as String,
        content: '[Unable to decrypt]',
        timestamp: data['timestamp'] as String,
      );
      state = state.copyWith(messages: [...state.messages, message]);
    }
  }

  /// Handle ReactionAdd event.
  void handleReactionAdd(Map<String, dynamic> event) {
    final data = event['data'] as Map<String, dynamic>;
    final messageId = data['message_id'] as String;
    final userId = data['user_id'] as String;
    final emoji = data['emoji'] as String;

    state = state.copyWith(
      messages: state.messages.map((msg) {
        if (msg.id != messageId) return msg;

        final hasGroup = msg.reactions.any((r) => r.emoji == emoji);
        List<ReactionGroup> reactions;

        if (hasGroup) {
          reactions = msg.reactions.map((r) {
            if (r.emoji != emoji) return r;
            if (r.userIds.contains(userId)) return r;
            final userIds = [...r.userIds, userId];
            return ReactionGroup(
                emoji: r.emoji, count: userIds.length, userIds: userIds);
          }).toList();
        } else {
          reactions = [
            ...msg.reactions,
            ReactionGroup(emoji: emoji, count: 1, userIds: [userId]),
          ];
        }

        return msg.copyWith(reactions: reactions);
      }).toList(),
    );
  }

  /// Handle ReactionRemove event.
  void handleReactionRemove(Map<String, dynamic> event) {
    final data = event['data'] as Map<String, dynamic>;
    final messageId = data['message_id'] as String;
    final userId = data['user_id'] as String;
    final emoji = data['emoji'] as String;

    state = state.copyWith(
      messages: state.messages.map((msg) {
        if (msg.id != messageId) return msg;

        final reactions = msg.reactions
            .map((r) {
              if (r.emoji != emoji) return r;
              final userIds =
                  r.userIds.where((id) => id != userId).toList();
              return ReactionGroup(
                  emoji: r.emoji, count: userIds.length, userIds: userIds);
            })
            .where((r) => r.count > 0)
            .toList();

        return msg.copyWith(reactions: reactions);
      }).toList(),
    );
  }

  /// Toggle a reaction via REST API.
  Future<void> toggleReaction(String messageId, String emoji) async {
    await _api.toggleReaction(
        HavenConstants.generalChannelId, messageId, emoji);
  }

  // -- Internal --

  Future<List<Message>> _decryptMessages(List<dynamic> raw) async {
    final messages = <Message>[];
    for (final msg in raw) {
      final map = msg as Map<String, dynamic>;
      try {
        final plaintext = await CryptoService.decrypt(
          channelKey,
          map['ciphertext'] as String,
          map['nonce'] as String,
        );
        final parsed = _parseContent(plaintext);
        messages.add(Message(
          id: map['id'] as String,
          channelId: map['channel_id'] as String,
          authorId: map['author_id'] as String,
          authorUsername: map['author_username'] as String,
          content: parsed['content'] as String,
          timestamp: map['created_at'] as String,
          reactions: (map['reactions'] as List<dynamic>?)
                  ?.map((r) =>
                      ReactionGroup.fromJson(r as Map<String, dynamic>))
                  .toList() ??
              [],
          imageData: parsed['imageData'] as String?,
          imageName: parsed['imageName'] as String?,
          fileId: parsed['fileId'] as String?,
          fileName: parsed['fileName'] as String?,
          fileSize: parsed['fileSize'] as int?,
        ));
      } catch (_) {
        messages.add(Message(
          id: map['id'] as String,
          channelId: map['channel_id'] as String,
          authorId: map['author_id'] as String,
          authorUsername: map['author_username'] as String,
          content: '[Unable to decrypt]',
          timestamp: map['created_at'] as String,
          reactions: (map['reactions'] as List<dynamic>?)
                  ?.map((r) =>
                      ReactionGroup.fromJson(r as Map<String, dynamic>))
                  .toList() ??
              [],
        ));
      }
    }
    return messages;
  }

  Map<String, dynamic> _parseContent(String plaintext) {
    try {
      final parsed = jsonDecode(plaintext) as Map<String, dynamic>;
      if (parsed['type'] == 'image' && parsed['data'] is String) {
        final mime = parsed['mime'] as String? ?? 'image/jpeg';
        return {
          'content': parsed['name'] as String? ?? 'image',
          'imageData': 'data:$mime;base64,${parsed['data']}',
          'imageName': parsed['name'] as String?,
        };
      }
      if (parsed['type'] == 'video' && parsed['file_id'] is String) {
        return {
          'content': parsed['name'] as String? ?? 'video',
        };
      }
      if (parsed['type'] == 'file' && parsed['file_id'] is String) {
        final size = parsed['size'] as int? ?? 0;
        return {
          'content': '${parsed['name'] ?? 'file'} (${formatFileSize(size)})',
          'fileId': parsed['file_id'] as String,
          'fileName': parsed['name'] as String? ?? 'file',
          'fileSize': size,
        };
      }
    } catch (_) {
      // Not JSON — plain text
    }
    return {'content': plaintext};
  }
}

final messageProvider =
    StateNotifierProvider<MessageNotifier, MessageState>((ref) {
  final api = ref.watch(apiServiceProvider);
  return MessageNotifier(api, channelKey: HavenConstants.defaultChannelKey);
});
