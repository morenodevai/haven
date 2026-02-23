import 'package:file_picker/file_picker.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import 'package:haven_app/config/constants.dart';
import 'package:haven_app/config/theme.dart';

/// Common image file extensions for inline image detection.
const _imageExtensions = {'.png', '.jpg', '.jpeg', '.gif', '.webp', '.bmp'};

bool _isImagePath(String path) {
  final lower = path.toLowerCase();
  return _imageExtensions.any((ext) => lower.endsWith(ext));
}

class MessageInput extends StatefulWidget {
  final Future<void> Function(String message) onSend;
  final VoidCallback? onTyping;
  final Future<void> Function(String filePath)? onFileAttach;
  final Future<void> Function(String filePath)? onImageAttach;

  const MessageInput({
    super.key,
    required this.onSend,
    this.onTyping,
    this.onFileAttach,
    this.onImageAttach,
  });

  @override
  State<MessageInput> createState() => _MessageInputState();
}

class _MessageInputState extends State<MessageInput> {
  final _controller = TextEditingController();
  late final FocusNode _focusNode;
  bool _isSending = false;
  DateTime? _lastTypingNotification;

  @override
  void initState() {
    super.initState();
    _focusNode = FocusNode(
      onKeyEvent: (node, event) {
        // Enter sends, Shift+Enter inserts newline
        if (event is KeyDownEvent &&
            event.logicalKey == LogicalKeyboardKey.enter &&
            !HardwareKeyboard.instance.isShiftPressed) {
          _send();
          return KeyEventResult.handled;
        }
        return KeyEventResult.ignored;
      },
    );
  }

  @override
  void dispose() {
    _controller.dispose();
    _focusNode.dispose();
    super.dispose();
  }

  void _onChanged(String text) {
    if (text.isNotEmpty && widget.onTyping != null) {
      final now = DateTime.now();
      if (_lastTypingNotification == null ||
          now.difference(_lastTypingNotification!) >
              HavenConstants.typingThrottle) {
        _lastTypingNotification = now;
        widget.onTyping!();
      }
    }
  }

  Future<void> _pickAndAttachFile() async {
    final result = await FilePicker.platform.pickFiles();
    if (result == null || result.files.isEmpty) return;

    final file = result.files.single;
    final path = file.path;
    if (path == null) return;

    try {
      // Route images to inline image handler if available
      if (_isImagePath(path) && widget.onImageAttach != null) {
        await widget.onImageAttach!(path);
      } else if (widget.onFileAttach != null) {
        await widget.onFileAttach!(path);
      }
    } catch (e) {
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text('Failed to send: $e')),
        );
      }
    }
  }

  Future<void> _pickImage() async {
    final result = await FilePicker.platform.pickFiles(
      type: FileType.image,
    );
    if (result == null || result.files.isEmpty) return;

    final path = result.files.single.path;
    if (path == null) return;

    if (widget.onImageAttach != null) {
      try {
        await widget.onImageAttach!(path);
      } catch (e) {
        if (mounted) {
          ScaffoldMessenger.of(context).showSnackBar(
            SnackBar(content: Text('Failed to send image: $e')),
          );
        }
      }
    }
  }

  Future<void> _send() async {
    final text = _controller.text.trim();
    if (text.isEmpty || _isSending) return;

    setState(() => _isSending = true);
    try {
      await widget.onSend(text);
      _controller.clear();
    } catch (e) {
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text('Failed to send: $e')),
        );
      }
    } finally {
      if (mounted) {
        setState(() => _isSending = false);
        _focusNode.requestFocus();
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.fromLTRB(16, 8, 16, 16),
      child: Row(
        children: [
          if (widget.onFileAttach != null)
            IconButton(
              onPressed: _pickAndAttachFile,
              icon: const Icon(Icons.attach_file_rounded),
              color: HavenTheme.textMuted,
              tooltip: 'Attach file',
            ),
          if (widget.onImageAttach != null)
            IconButton(
              onPressed: _pickImage,
              icon: const Icon(Icons.image_outlined),
              color: HavenTheme.textMuted,
              tooltip: 'Send image',
            ),
          Expanded(
            child: TextField(
              controller: _controller,
              focusNode: _focusNode,
              onChanged: _onChanged,
              maxLines: 5,
              minLines: 1,
              textInputAction: TextInputAction.newline,
              decoration: InputDecoration(
                hintText: 'Send an encrypted message...',
                filled: true,
                fillColor: HavenTheme.inputBackground,
                border: OutlineInputBorder(
                  borderRadius: BorderRadius.circular(12),
                  borderSide: BorderSide.none,
                ),
                contentPadding: const EdgeInsets.symmetric(
                  horizontal: 16,
                  vertical: 12,
                ),
              ),
              style: const TextStyle(
                fontSize: 14,
                color: HavenTheme.textPrimary,
              ),
            ),
          ),
          const SizedBox(width: 8),
          IconButton(
            onPressed: _isSending ? null : _send,
            icon: _isSending
                ? const SizedBox(
                    width: 20,
                    height: 20,
                    child: CircularProgressIndicator(strokeWidth: 2),
                  )
                : const Icon(Icons.send_rounded),
            color: HavenTheme.primaryLight,
            tooltip: 'Send message',
          ),
        ],
      ),
    );
  }
}
