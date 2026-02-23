import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import 'package:haven_app/config/theme.dart';
import 'package:haven_app/providers/voice_provider.dart';

class VoiceControls extends ConsumerWidget {
  const VoiceControls({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final voiceState = ref.watch(voiceProvider);

    // Show error if any
    if (voiceState.error != null) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(
            content: Text(voiceState.error!),
            backgroundColor: HavenTheme.error,
          ),
        );
      });
    }

    if (!voiceState.isInVoice) {
      return Center(
        child: Column(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            Icon(Icons.headset, size: 64, color: HavenTheme.textMuted),
            const SizedBox(height: 16),
            Text(
              'Voice Channel',
              style: TextStyle(
                fontSize: 18,
                color: HavenTheme.textSecondary,
              ),
            ),
            const SizedBox(height: 8),
            Text(
              'Click to join voice chat',
              style: TextStyle(
                fontSize: 14,
                color: HavenTheme.textMuted,
              ),
            ),
            const SizedBox(height: 24),
            ElevatedButton.icon(
              onPressed: () {
                ref.read(voiceProvider.notifier).joinVoice();
              },
              icon: const Icon(Icons.call),
              label: const Text('Join Voice'),
            ),
          ],
        ),
      );
    }

    return Column(
      children: [
        // Participants
        Expanded(
          child: ListView(
            padding: const EdgeInsets.all(16),
            children: voiceState.participants.values.map((p) {
              return Card(
                color: HavenTheme.surface,
                margin: const EdgeInsets.only(bottom: 8),
                child: ListTile(
                  leading: CircleAvatar(
                    backgroundColor: p.speaking
                        ? HavenTheme.online
                        : HavenTheme.surfaceVariant,
                    child: Text(
                      p.username.isNotEmpty
                          ? p.username[0].toUpperCase()
                          : '?',
                      style: const TextStyle(color: Colors.white),
                    ),
                  ),
                  title: Text(p.username,
                      style:
                          const TextStyle(color: HavenTheme.textPrimary)),
                  trailing: Row(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      if (p.selfMute)
                        const Icon(Icons.mic_off, color: HavenTheme.error,
                            size: 18),
                      if (p.selfDeaf) ...[
                        const SizedBox(width: 4),
                        const Icon(Icons.headset_off,
                            color: HavenTheme.error, size: 18),
                      ],
                      if (p.speaking)
                        const Icon(Icons.graphic_eq,
                            color: HavenTheme.online, size: 18),
                    ],
                  ),
                ),
              );
            }).toList(),
          ),
        ),

        // Control bar
        Container(
          padding: const EdgeInsets.all(16),
          decoration: const BoxDecoration(
            border: Border(top: BorderSide(color: HavenTheme.divider)),
          ),
          child: Row(
            mainAxisAlignment: MainAxisAlignment.center,
            children: [
              _ControlButton(
                icon: voiceState.selfMute ? Icons.mic_off : Icons.mic,
                label: voiceState.selfMute ? 'Unmute' : 'Mute',
                isActive: voiceState.selfMute,
                onTap: () {
                  ref.read(voiceProvider.notifier).toggleMute();
                },
              ),
              const SizedBox(width: 16),
              _ControlButton(
                icon: voiceState.selfDeaf
                    ? Icons.headset_off
                    : Icons.headset,
                label: voiceState.selfDeaf ? 'Undeafen' : 'Deafen',
                isActive: voiceState.selfDeaf,
                onTap: () {
                  ref.read(voiceProvider.notifier).toggleDeaf();
                },
              ),
              const SizedBox(width: 16),
              _ControlButton(
                icon: Icons.call_end,
                label: 'Leave',
                isActive: true,
                activeColor: HavenTheme.error,
                onTap: () {
                  ref.read(voiceProvider.notifier).leaveVoice();
                },
              ),
            ],
          ),
        ),
      ],
    );
  }
}

class _ControlButton extends StatelessWidget {
  final IconData icon;
  final String label;
  final bool isActive;
  final Color? activeColor;
  final VoidCallback onTap;

  const _ControlButton({
    required this.icon,
    required this.label,
    required this.isActive,
    this.activeColor,
    required this.onTap,
  });

  @override
  Widget build(BuildContext context) {
    final color = isActive
        ? (activeColor ?? HavenTheme.error)
        : HavenTheme.textSecondary;

    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        Material(
          color: isActive
              ? color.withValues(alpha: 0.2)
              : HavenTheme.surface,
          shape: const CircleBorder(),
          child: InkWell(
            onTap: onTap,
            customBorder: const CircleBorder(),
            child: Padding(
              padding: const EdgeInsets.all(16),
              child: Icon(icon, color: color, size: 24),
            ),
          ),
        ),
        const SizedBox(height: 4),
        Text(
          label,
          style: TextStyle(fontSize: 11, color: HavenTheme.textMuted),
        ),
      ],
    );
  }
}
