import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import 'package:haven_app/config/theme.dart';
import 'package:haven_app/models/message.dart';
import 'package:haven_app/utils/formatters.dart';
import 'package:haven_app/widgets/emoji_picker.dart';

class MessageBubble extends StatefulWidget {
  final Message message;
  final bool isOwnMessage;
  final String? currentUserId;
  final void Function(String messageId, String emoji)? onReactionTap;
  final void Function(String fileId, String fileName)? onFileDownload;

  const MessageBubble({
    super.key,
    required this.message,
    required this.isOwnMessage,
    this.currentUserId,
    this.onReactionTap,
    this.onFileDownload,
  });

  @override
  State<MessageBubble> createState() => _MessageBubbleState();
}

class _MessageBubbleState extends State<MessageBubble>
    with AutomaticKeepAliveClientMixin {
  bool _isHovered = false;
  Uint8List? _decodedImageBytes;

  @override
  bool get wantKeepAlive => _decodedImageBytes != null;

  @override
  void initState() {
    super.initState();
    _decodeImageData();
  }

  @override
  void didUpdateWidget(MessageBubble oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.message.imageData != widget.message.imageData) {
      _decodeImageData();
      updateKeepAlive();
    }
  }

  void _decodeImageData() {
    final dataUri = widget.message.imageData;
    if (dataUri == null) {
      _decodedImageBytes = null;
      return;
    }
    try {
      final commaIndex = dataUri.indexOf(',');
      if (commaIndex < 0) {
        _decodedImageBytes = null;
        return;
      }
      _decodedImageBytes = base64.decode(dataUri.substring(commaIndex + 1));
    } catch (_) {
      _decodedImageBytes = null;
    }
  }

  void _showEmojiPicker(BuildContext context) {
    final RenderBox box = context.findRenderObject() as RenderBox;
    final offset = box.localToGlobal(Offset.zero);

    showDialog(
      context: context,
      barrierColor: Colors.transparent,
      builder: (dialogContext) {
        return Stack(
          children: [
            // Dismiss on tap outside
            Positioned.fill(
              child: GestureDetector(
                onTap: () => Navigator.of(dialogContext).pop(),
                behavior: HitTestBehavior.opaque,
                child: const SizedBox.expand(),
              ),
            ),
            Positioned(
              left: offset.dx + 46,
              top: offset.dy - 48,
              child: Material(
                color: Colors.transparent,
                child: EmojiPicker(
                  onEmojiSelected: (emoji) {
                    Navigator.of(dialogContext).pop();
                    widget.onReactionTap?.call(widget.message.id, emoji);
                  },
                ),
              ),
            ),
          ],
        );
      },
    );
  }

  bool get _hasTextContent =>
      !widget.message.isFileMessage &&
      widget.message.content != widget.message.imageName &&
      widget.message.content != 'image';

  @override
  Widget build(BuildContext context) {
    super.build(context);
    return MouseRegion(
      onEnter: (_) => setState(() => _isHovered = true),
      onExit: (_) => setState(() => _isHovered = false),
      child: Stack(
        clipBehavior: Clip.none,
        children: [
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 3),
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                // Avatar
                Container(
                  width: 36,
                  height: 36,
                  decoration: BoxDecoration(
                    color: widget.isOwnMessage
                        ? HavenTheme.primaryLight
                        : HavenTheme.surfaceVariant,
                    borderRadius: BorderRadius.circular(18),
                  ),
                  child: Center(
                    child: Text(
                      widget.message.authorUsername.isNotEmpty
                          ? widget.message.authorUsername[0].toUpperCase()
                          : '?',
                      style: const TextStyle(
                        color: Colors.white,
                        fontWeight: FontWeight.bold,
                        fontSize: 14,
                      ),
                    ),
                  ),
                ),
                const SizedBox(width: 10),

                // Message content
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      // Author + timestamp
                      Row(
                        children: [
                          Text(
                            widget.message.authorUsername,
                            style: TextStyle(
                              fontSize: 13,
                              fontWeight: FontWeight.w600,
                              color: widget.isOwnMessage
                                  ? HavenTheme.primaryLight
                                  : HavenTheme.textPrimary,
                            ),
                          ),
                          const SizedBox(width: 8),
                          Text(
                            formatTimestamp(widget.message.timestamp),
                            style: TextStyle(
                              fontSize: 11,
                              color: HavenTheme.textMuted,
                            ),
                          ),
                        ],
                      ),
                      const SizedBox(height: 2),

                      // Inline image
                      if (_decodedImageBytes != null) ...[
                        _buildImage(),
                        const SizedBox(height: 4),
                      ],

                      // File attachment
                      if (widget.message.isFileMessage) ...[
                        _buildFileCard(),
                        const SizedBox(height: 4),
                      ],

                      // Text content (hide for pure file/image messages)
                      if (_hasTextContent) ...[
                        Text(
                          widget.message.content,
                          style: const TextStyle(
                            fontSize: 14,
                            color: HavenTheme.textPrimary,
                            height: 1.4,
                          ),
                        ),
                      ],

                      // Reactions
                      if (widget.message.reactions.isNotEmpty) ...[
                        const SizedBox(height: 4),
                        Wrap(
                          spacing: 4,
                          runSpacing: 4,
                          children: widget.message.reactions.map((reaction) {
                            final isMine = widget.currentUserId != null &&
                                reaction.userIds.contains(widget.currentUserId);
                            return _ReactionBadge(
                              emoji: reaction.emoji,
                              count: reaction.count,
                              isMine: isMine,
                              onTap: () {
                                widget.onReactionTap
                                    ?.call(widget.message.id, reaction.emoji);
                              },
                            );
                          }).toList(),
                        ),
                      ],
                    ],
                  ),
                ),
              ],
            ),
          ),

          // Hover action bar (reaction button)
          if (_isHovered)
            Positioned(
              top: 0,
              right: 24,
              child: Container(
                decoration: BoxDecoration(
                  color: HavenTheme.surface,
                  borderRadius: BorderRadius.circular(6),
                  border: Border.all(color: HavenTheme.divider),
                ),
                child: Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    if (_hasTextContent)
                      _HoverActionButton(
                        icon: Icons.copy_rounded,
                        tooltip: 'Copy',
                        onTap: () {
                          Clipboard.setData(
                              ClipboardData(text: widget.message.content));
                        },
                      ),
                    _HoverActionButton(
                      icon: Icons.add_reaction_outlined,
                      tooltip: 'Add reaction',
                      onTap: () => _showEmojiPicker(context),
                    ),
                  ],
                ),
              ),
            ),
        ],
      ),
    );
  }

  Widget _buildFileCard() {
    return Container(
      constraints: const BoxConstraints(maxWidth: 320),
      padding: const EdgeInsets.all(12),
      decoration: BoxDecoration(
        color: HavenTheme.surface,
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: HavenTheme.divider),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Container(
            width: 40,
            height: 40,
            decoration: BoxDecoration(
              color: HavenTheme.primaryLight.withValues(alpha: 0.2),
              borderRadius: BorderRadius.circular(8),
            ),
            child: const Icon(Icons.insert_drive_file,
                color: HavenTheme.primaryLight, size: 22),
          ),
          const SizedBox(width: 12),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  widget.message.fileName ?? 'file',
                  style: const TextStyle(
                    fontSize: 13,
                    fontWeight: FontWeight.w500,
                    color: HavenTheme.textPrimary,
                  ),
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                ),
                Text(
                  widget.message.content,
                  style: TextStyle(
                    fontSize: 11,
                    color: HavenTheme.textMuted,
                  ),
                ),
              ],
            ),
          ),
          const SizedBox(width: 8),
          IconButton(
            icon: const Icon(Icons.download_rounded, size: 20),
            onPressed: () {
              widget.onFileDownload?.call(
                  widget.message.fileId!, widget.message.fileName ?? 'file');
            },
            color: HavenTheme.primaryLight,
            tooltip: 'Download',
            constraints: const BoxConstraints(minWidth: 36, minHeight: 36),
            padding: EdgeInsets.zero,
          ),
        ],
      ),
    );
  }

  Widget _buildImage() {
    return ClipRRect(
      borderRadius: BorderRadius.circular(8),
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 400, maxHeight: 300),
        child: Image.memory(
          _decodedImageBytes!,
          fit: BoxFit.contain,
          gaplessPlayback: true,
          errorBuilder: (context, error, stackTrace) => Container(
            padding: const EdgeInsets.all(8),
            decoration: BoxDecoration(
              color: HavenTheme.surface,
              borderRadius: BorderRadius.circular(8),
            ),
            child: const Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                Icon(Icons.broken_image, size: 16),
                SizedBox(width: 4),
                Text('Failed to load image'),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class _HoverActionButton extends StatelessWidget {
  final IconData icon;
  final String tooltip;
  final VoidCallback onTap;

  const _HoverActionButton({
    required this.icon,
    required this.tooltip,
    required this.onTap,
  });

  @override
  Widget build(BuildContext context) {
    return IconButton(
      icon: Icon(icon, size: 16),
      onPressed: onTap,
      tooltip: tooltip,
      constraints: const BoxConstraints(minWidth: 30, minHeight: 30),
      padding: const EdgeInsets.all(4),
      iconSize: 16,
      color: HavenTheme.textMuted,
      hoverColor: HavenTheme.primaryLight.withValues(alpha: 0.1),
    );
  }
}

class _ReactionBadge extends StatelessWidget {
  final String emoji;
  final int count;
  final bool isMine;
  final VoidCallback onTap;

  const _ReactionBadge({
    required this.emoji,
    required this.count,
    required this.isMine,
    required this.onTap,
  });

  @override
  Widget build(BuildContext context) {
    return Material(
      color: isMine
          ? HavenTheme.primaryLight.withValues(alpha: 0.3)
          : HavenTheme.surface,
      borderRadius: BorderRadius.circular(12),
      child: InkWell(
        onTap: onTap,
        borderRadius: BorderRadius.circular(12),
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Text(emoji, style: const TextStyle(fontSize: 14)),
              if (count > 1) ...[
                const SizedBox(width: 4),
                Text(
                  '$count',
                  style: TextStyle(
                    fontSize: 12,
                    color: isMine
                        ? HavenTheme.primaryLight
                        : HavenTheme.textMuted,
                  ),
                ),
              ],
            ],
          ),
        ),
      ),
    );
  }
}
