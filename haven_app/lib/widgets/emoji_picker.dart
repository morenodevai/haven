import 'package:flutter/material.dart';

import 'package:haven_app/config/theme.dart';

/// A compact emoji picker popup for message reactions.
class EmojiPicker extends StatelessWidget {
  final void Function(String emoji) onEmojiSelected;

  const EmojiPicker({super.key, required this.onEmojiSelected});

  // Common reaction emojis — small curated set matching typical chat apps
  static const List<String> emojis = [
    '\u{1F44D}', // 👍
    '\u{1F44E}', // 👎
    '\u{2764}', // ❤️
    '\u{1F602}', // 😂
    '\u{1F62E}', // 😮
    '\u{1F622}', // 😢
    '\u{1F525}', // 🔥
    '\u{1F389}', // 🎉
    '\u{1F914}', // 🤔
    '\u{1F44F}', // 👏
    '\u{1F64F}', // 🙏
    '\u{1F680}', // 🚀
  ];

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.all(8),
      decoration: BoxDecoration(
        color: HavenTheme.surface,
        borderRadius: BorderRadius.circular(12),
        border: Border.all(color: HavenTheme.divider),
        boxShadow: [
          BoxShadow(
            color: Colors.black.withValues(alpha: 0.3),
            blurRadius: 8,
            offset: const Offset(0, 2),
          ),
        ],
      ),
      child: Wrap(
        spacing: 2,
        runSpacing: 2,
        children: emojis.map((emoji) {
          return InkWell(
            onTap: () => onEmojiSelected(emoji),
            borderRadius: BorderRadius.circular(8),
            child: Padding(
              padding: const EdgeInsets.all(6),
              child: Text(emoji, style: const TextStyle(fontSize: 20)),
            ),
          );
        }).toList(),
      ),
    );
  }
}
