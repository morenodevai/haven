import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import 'package:haven_app/config/theme.dart';
import 'package:haven_app/providers/audio_settings_provider.dart';
import 'package:haven_app/providers/video_provider.dart';
import 'package:haven_app/providers/voice_provider.dart';

class VoiceControls extends ConsumerWidget {
  const VoiceControls({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final voiceState = ref.watch(voiceProvider);
    final videoState = ref.watch(videoProvider);

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
              final userVol = ref.watch(audioSettingsProvider).userVolumes[p.userId] ?? 1.0;
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
                      const SizedBox(width: 4),
                      _VolumeButton(userId: p.userId, volume: userVol),
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
                icon: videoState.cameraEnabled
                    ? Icons.videocam
                    : Icons.videocam_off,
                label: videoState.cameraEnabled ? 'Cam Off' : 'Camera',
                isActive: videoState.cameraEnabled,
                activeColor: HavenTheme.online,
                onTap: () {
                  ref.read(videoProvider.notifier).toggleCamera();
                },
              ),
              const SizedBox(width: 16),
              _ControlButton(
                icon: videoState.screenShareEnabled
                    ? Icons.stop_screen_share
                    : Icons.screen_share,
                label: videoState.screenShareEnabled ? 'Stop Share' : 'Share',
                isActive: videoState.screenShareEnabled,
                activeColor: HavenTheme.online,
                onTap: () {
                  ref.read(videoProvider.notifier).toggleScreenShare();
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

class _VolumeButton extends ConsumerWidget {
  final String userId;
  final double volume;

  const _VolumeButton({required this.userId, required this.volume});

  IconData _volumeIcon(double vol) {
    if (vol == 0) return Icons.volume_off;
    if (vol < 1.0) return Icons.volume_down;
    return Icons.volume_up;
  }

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    return PopupMenuButton<void>(
      tooltip: '${(volume * 100).round()}%',
      padding: EdgeInsets.zero,
      constraints: const BoxConstraints(minWidth: 180),
      color: HavenTheme.surface,
      icon: Icon(
        _volumeIcon(volume),
        size: 18,
        color: volume == 0
            ? HavenTheme.error
            : volume == 1.0
                ? HavenTheme.textMuted
                : HavenTheme.primaryLight,
      ),
      itemBuilder: (_) => [
        PopupMenuItem<void>(
          enabled: false,
          child: _VolumeSliderContent(userId: userId, volume: volume),
        ),
      ],
    );
  }
}

class _VolumeSliderContent extends ConsumerStatefulWidget {
  final String userId;
  final double volume;

  const _VolumeSliderContent({required this.userId, required this.volume});

  @override
  ConsumerState<_VolumeSliderContent> createState() =>
      _VolumeSliderContentState();
}

class _VolumeSliderContentState extends ConsumerState<_VolumeSliderContent> {
  late double _vol;

  @override
  void initState() {
    super.initState();
    _vol = widget.volume;
  }

  @override
  Widget build(BuildContext context) {
    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        Row(
          children: [
            Text(
              '${(_vol * 100).round()}%',
              style: const TextStyle(
                fontSize: 13,
                color: HavenTheme.textPrimary,
                fontWeight: FontWeight.w500,
              ),
            ),
            const Spacer(),
            InkWell(
              onTap: () {
                setState(() => _vol = 0);
                ref.read(audioSettingsProvider.notifier).setUserVolume(widget.userId, 0);
              },
              child: const Padding(
                padding: EdgeInsets.all(4),
                child: Icon(Icons.volume_off, size: 16, color: HavenTheme.error),
              ),
            ),
            const SizedBox(width: 8),
            InkWell(
              onTap: () {
                setState(() => _vol = 1.0);
                ref.read(audioSettingsProvider.notifier).setUserVolume(widget.userId, 1.0);
              },
              child: const Padding(
                padding: EdgeInsets.all(4),
                child: Icon(Icons.restart_alt, size: 16, color: HavenTheme.textMuted),
              ),
            ),
          ],
        ),
        SliderTheme(
          data: SliderThemeData(
            activeTrackColor: HavenTheme.primaryLight,
            inactiveTrackColor: HavenTheme.surfaceVariant,
            thumbColor: HavenTheme.primaryLight,
            overlayColor: HavenTheme.primaryLight.withValues(alpha: 0.2),
            trackHeight: 3,
            thumbShape: const RoundSliderThumbShape(enabledThumbRadius: 6),
          ),
          child: Slider(
            value: _vol,
            min: 0,
            max: 2.0,
            onChanged: (v) {
              setState(() => _vol = v);
              ref.read(audioSettingsProvider.notifier).setUserVolume(widget.userId, v);
            },
          ),
        ),
      ],
    );
  }
}
