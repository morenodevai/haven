import 'package:flutter/material.dart';

import 'package:haven_app/config/theme.dart';

class OnlineIndicator extends StatelessWidget {
  final bool isOnline;
  final double size;

  const OnlineIndicator({
    super.key,
    required this.isOnline,
    this.size = 10,
  });

  @override
  Widget build(BuildContext context) {
    return Container(
      width: size,
      height: size,
      decoration: BoxDecoration(
        color: isOnline ? HavenTheme.online : HavenTheme.textMuted,
        borderRadius: BorderRadius.circular(size / 2),
        border: Border.all(
          color: HavenTheme.background,
          width: 2,
        ),
      ),
    );
  }
}
