import 'dart:convert';
import 'dart:io';

import 'package:file_picker/file_picker.dart';
import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import 'package:haven_app/config/constants.dart';
import 'package:haven_app/config/theme.dart';
import 'package:haven_app/providers/auth_provider.dart';
import 'package:haven_app/providers/gateway_provider.dart';
import 'package:haven_app/providers/message_provider.dart';
import 'package:haven_app/providers/typing_provider.dart';
import 'package:haven_app/widgets/message_bubble.dart';
import 'package:haven_app/widgets/message_input.dart';

class ChatScreen extends ConsumerStatefulWidget {
  const ChatScreen({super.key});

  @override
  ConsumerState<ChatScreen> createState() => _ChatScreenState();
}

class _ChatScreenState extends ConsumerState<ChatScreen> {
  final _scrollController = ScrollController();

  @override
  void initState() {
    super.initState();
    _scrollController.addListener(_onScroll);
  }

  @override
  void dispose() {
    _scrollController.removeListener(_onScroll);
    _scrollController.dispose();
    super.dispose();
  }

  void _onScroll() {
    // Load more when scrolled to top
    if (_scrollController.position.pixels <=
        _scrollController.position.minScrollExtent + 100) {
      ref.read(messageProvider.notifier).loadMoreMessages();
    }
  }

  void _scrollToBottom() {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (_scrollController.hasClients) {
        _scrollController.animateTo(
          _scrollController.position.maxScrollExtent,
          duration: const Duration(milliseconds: 200),
          curve: Curves.easeOut,
        );
      }
    });
  }

  @override
  Widget build(BuildContext context) {
    final messageState = ref.watch(messageProvider);
    final authState = ref.watch(authProvider);
    final typingUsers = ref.watch(typingProvider);

    // Auto-scroll on new messages
    ref.listen<MessageState>(messageProvider, (prev, next) {
      if (prev != null && next.messages.length > prev.messages.length) {
        _scrollToBottom();
      }
    });

    return Column(
      children: [
        // Channel header
        Container(
          padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
          decoration: const BoxDecoration(
            color: HavenTheme.surface,
            border: Border(
              bottom: BorderSide(color: HavenTheme.divider),
            ),
          ),
          child: Row(
            children: [
              const Icon(Icons.tag, size: 20, color: HavenTheme.textMuted),
              const SizedBox(width: 8),
              const Text(
                'general',
                style: TextStyle(
                  fontSize: 16,
                  fontWeight: FontWeight.w600,
                  color: HavenTheme.textPrimary,
                ),
              ),
              const SizedBox(width: 8),
              Container(
                width: 1,
                height: 20,
                color: HavenTheme.divider,
              ),
              const SizedBox(width: 8),
              Text(
                'End-to-end encrypted',
                style: TextStyle(
                  fontSize: 13,
                  color: HavenTheme.textMuted,
                ),
              ),
              const Spacer(),
              Icon(Icons.lock_outline, size: 16, color: HavenTheme.online),
            ],
          ),
        ),

        // Messages
        Expanded(
          child: messageState.isLoading && messageState.messages.isEmpty
              ? const Center(child: CircularProgressIndicator())
              : messageState.messages.isEmpty
                  ? Center(
                      child: Column(
                        mainAxisAlignment: MainAxisAlignment.center,
                        children: [
                          Icon(Icons.chat_bubble_outline,
                              size: 48, color: HavenTheme.textMuted),
                          const SizedBox(height: 16),
                          Text(
                            'No messages yet',
                            style: TextStyle(
                              color: HavenTheme.textMuted,
                              fontSize: 16,
                            ),
                          ),
                          const SizedBox(height: 8),
                          Text(
                            'Send the first encrypted message!',
                            style: TextStyle(
                              color: HavenTheme.textMuted,
                              fontSize: 13,
                            ),
                          ),
                        ],
                      ),
                    )
                  : ListView.builder(
                      controller: _scrollController,
                      cacheExtent: 1000,
                      padding: const EdgeInsets.symmetric(vertical: 8),
                      itemCount: messageState.messages.length,
                      itemBuilder: (context, index) {
                        final message = messageState.messages[index];
                        return MessageBubble(
                          message: message,
                          isOwnMessage:
                              message.authorId == authState.userId,
                          currentUserId: authState.userId,
                          onReactionTap: (messageId, emoji) {
                            ref
                                .read(messageProvider.notifier)
                                .toggleReaction(messageId, emoji);
                          },
                          onFileDownload: (fileId, fileName) {
                            _downloadFile(context, ref, fileId, fileName);
                          },
                        );
                      },
                    ),
        ),

        // Typing indicator
        if (typingUsers.isNotEmpty)
          Container(
            padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 4),
            alignment: Alignment.centerLeft,
            child: Text(
              _buildTypingText(typingUsers.values.toList()),
              style: TextStyle(
                fontSize: 12,
                color: HavenTheme.textMuted,
                fontStyle: FontStyle.italic,
              ),
            ),
          ),

        // Message input
        MessageInput(
          onSend: (content) async {
            await ref.read(messageProvider.notifier).sendMessage(content);
          },
          onTyping: () {
            final gateway = ref.read(gatewayServiceProvider);
            gateway.startTyping(HavenConstants.generalChannelId);
          },
          onFileAttach: (filePath) async {
            await ref
                .read(messageProvider.notifier)
                .sendFileMessage(filePath);
          },
          onImageAttach: (filePath) async {
            await _sendImage(filePath);
          },
        ),
      ],
    );
  }

  Future<void> _sendImage(String filePath) async {
    final file = File(filePath);
    final name = file.uri.pathSegments.last;
    final ext = name.toLowerCase().split('.').last;
    final mimeMap = {
      'png': 'image/png',
      'jpg': 'image/jpeg',
      'jpeg': 'image/jpeg',
      'gif': 'image/gif',
      'webp': 'image/webp',
      'bmp': 'image/bmp',
    };
    final mime = mimeMap[ext] ?? 'image/jpeg';
    final bytes = await file.readAsBytes();
    final b64 = base64.encode(bytes);

    await ref.read(messageProvider.notifier).sendImageMessage(
          name: name,
          mime: mime,
          base64Data: b64,
        );
  }

  Future<void> _downloadFile(BuildContext context, WidgetRef ref,
      String fileId, String fileName) async {
    // Let user choose save location
    final savePath = await FilePicker.platform.saveFile(
      dialogTitle: 'Save file',
      fileName: fileName,
    );
    if (savePath == null) return;

    try {
      if (context.mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text('Downloading $fileName...')),
        );
      }
      await ref
          .read(messageProvider.notifier)
          .downloadAndSaveFile(fileId, savePath);
      if (context.mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text('Saved $fileName')),
        );
      }
    } catch (e) {
      if (context.mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text('Download failed: $e')),
        );
      }
    }
  }

  String _buildTypingText(List<String> names) {
    if (names.length == 1) {
      return '${names[0]} is typing...';
    } else if (names.length == 2) {
      return '${names[0]} and ${names[1]} are typing...';
    } else {
      return 'Several people are typing...';
    }
  }
}
