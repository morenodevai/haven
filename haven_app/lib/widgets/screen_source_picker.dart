import 'package:flutter/material.dart';
import 'package:flutter_webrtc/flutter_webrtc.dart';

import 'package:haven_app/config/theme.dart';

class ScreenSourcePickerDialog extends StatelessWidget {
  final List<DesktopCapturerSource> sources;

  const ScreenSourcePickerDialog({super.key, required this.sources});

  @override
  Widget build(BuildContext context) {
    final screens = sources.where((s) => s.type == SourceType.Screen).toList();
    final windows = sources.where((s) => s.type == SourceType.Window).toList();

    return Dialog(
      backgroundColor: HavenTheme.surface,
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(12)),
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 600, maxHeight: 500),
        child: Padding(
          padding: const EdgeInsets.all(20),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Row(
                children: [
                  const Icon(Icons.screen_share, color: HavenTheme.textPrimary, size: 20),
                  const SizedBox(width: 8),
                  const Text(
                    'Share your screen',
                    style: TextStyle(
                      fontSize: 16,
                      fontWeight: FontWeight.w600,
                      color: HavenTheme.textPrimary,
                    ),
                  ),
                  const Spacer(),
                  IconButton(
                    icon: const Icon(Icons.close, size: 18),
                    onPressed: () => Navigator.of(context).pop(),
                    color: HavenTheme.textMuted,
                  ),
                ],
              ),
              const SizedBox(height: 16),
              if (screens.isNotEmpty) ...[
                const Text(
                  'SCREENS',
                  style: TextStyle(
                    fontSize: 11,
                    fontWeight: FontWeight.w600,
                    color: HavenTheme.textMuted,
                    letterSpacing: 1.2,
                  ),
                ),
                const SizedBox(height: 8),
                Wrap(
                  spacing: 8,
                  runSpacing: 8,
                  children: screens.map((s) => SourceTile(
                    source: s,
                    onTap: () => Navigator.of(context).pop(s),
                  )).toList(),
                ),
                const SizedBox(height: 16),
              ],
              if (windows.isNotEmpty) ...[
                const Text(
                  'WINDOWS',
                  style: TextStyle(
                    fontSize: 11,
                    fontWeight: FontWeight.w600,
                    color: HavenTheme.textMuted,
                    letterSpacing: 1.2,
                  ),
                ),
                const SizedBox(height: 8),
                Expanded(
                  child: GridView.builder(
                    gridDelegate: const SliverGridDelegateWithFixedCrossAxisCount(
                      crossAxisCount: 3,
                      mainAxisSpacing: 8,
                      crossAxisSpacing: 8,
                      childAspectRatio: 16 / 10,
                    ),
                    itemCount: windows.length,
                    itemBuilder: (context, i) => SourceTile(
                      source: windows[i],
                      onTap: () => Navigator.of(context).pop(windows[i]),
                    ),
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

class SourceTile extends StatelessWidget {
  final DesktopCapturerSource source;
  final VoidCallback onTap;

  const SourceTile({super.key, required this.source, required this.onTap});

  @override
  Widget build(BuildContext context) {
    return InkWell(
      onTap: onTap,
      borderRadius: BorderRadius.circular(8),
      child: Container(
        width: 160,
        height: 100,
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(8),
          border: Border.all(color: HavenTheme.divider),
          color: HavenTheme.sidebarBackground,
        ),
        clipBehavior: Clip.antiAlias,
        child: Column(
          children: [
            Expanded(
              child: source.thumbnail != null
                  ? Image.memory(
                      source.thumbnail!,
                      fit: BoxFit.cover,
                      width: double.infinity,
                    )
                  : const Center(
                      child: Icon(Icons.desktop_windows,
                          color: HavenTheme.textMuted, size: 32),
                    ),
            ),
            Container(
              width: double.infinity,
              padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 4),
              color: HavenTheme.surface,
              child: Text(
                source.name,
                style: const TextStyle(
                  fontSize: 11,
                  color: HavenTheme.textSecondary,
                ),
                overflow: TextOverflow.ellipsis,
                maxLines: 1,
              ),
            ),
          ],
        ),
      ),
    );
  }
}
