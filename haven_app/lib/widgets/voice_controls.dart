import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import 'package:haven_app/config/theme.dart';
import 'package:haven_app/providers/audio_settings_provider.dart';
import 'package:haven_app/providers/video_provider.dart';
import 'package:haven_app/providers/voice_provider.dart';
import 'package:haven_app/widgets/video_grid.dart';

class VoiceControls extends ConsumerWidget {
  const VoiceControls({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final voiceState = ref.watch(voiceProvider);
    final videoState = ref.watch(videoProvider);

    ref.listen<VoiceState>(voiceProvider, (prev, next) {
      if (next.error != null && prev?.error != next.error) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(
            content: Text(next.error!),
            backgroundColor: HavenTheme.error,
          ),
        );
      }
    });

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

    final showVideoGrid = videoState.hasAnyVideo && videoState.videoMode == VideoMode.expanded;

    return Column(
      children: [
        // Main area: video grid or participant list
        Expanded(
          child: showVideoGrid
              ? _VideoGrid(videoState: videoState, voiceState: voiceState)
              : _ParticipantList(voiceState: voiceState),
        ),

        // Compact participant strip (when video is showing)
        if (showVideoGrid)
          _CompactParticipantStrip(voiceState: voiceState),

        // Control bar
        _ControlBar(voiceState: voiceState, videoState: videoState),
      ],
    );
  }
}

// ─── Video Grid ──────────────────────────────────────────────────────────────

class _VideoGrid extends StatelessWidget {
  final VideoState videoState;
  final VoiceState voiceState;

  const _VideoGrid({required this.videoState, required this.voiceState});

  @override
  Widget build(BuildContext context) {
    final tiles = collectVideoTiles(videoState, voiceState);
    return VideoGrid(
      tiles: tiles,
      focusedStreamId: videoState.focusedStreamId,
      emptyText: 'No video',
    );
  }
}

// ─── Compact Participant Strip ───────────────────────────────────────────────

class _CompactParticipantStrip extends StatelessWidget {
  final VoiceState voiceState;

  const _CompactParticipantStrip({required this.voiceState});

  @override
  Widget build(BuildContext context) {
    return Container(
      height: 52,
      padding: const EdgeInsets.symmetric(horizontal: 8),
      decoration: const BoxDecoration(
        color: HavenTheme.surface,
        border: Border(top: BorderSide(color: HavenTheme.divider)),
      ),
      child: ListView(
        scrollDirection: Axis.horizontal,
        children: voiceState.participants.values.map((p) {
          return Padding(
            padding: const EdgeInsets.symmetric(horizontal: 4, vertical: 8),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                Container(
                  decoration: BoxDecoration(
                    shape: BoxShape.circle,
                    border: p.speaking
                        ? Border.all(color: HavenTheme.online, width: 2)
                        : null,
                  ),
                  child: CircleAvatar(
                    radius: 14,
                    backgroundColor: HavenTheme.surfaceVariant,
                    child: Text(
                      p.username.isNotEmpty ? p.username[0].toUpperCase() : '?',
                      style: const TextStyle(color: Colors.white, fontSize: 11),
                    ),
                  ),
                ),
                const SizedBox(width: 4),
                Text(
                  p.username,
                  style: TextStyle(
                    fontSize: 12,
                    color: p.speaking ? HavenTheme.online : HavenTheme.textSecondary,
                  ),
                ),
              ],
            ),
          );
        }).toList(),
      ),
    );
  }
}

// ─── Participant List (no video) ─────────────────────────────────────────────

class _ParticipantList extends ConsumerWidget {
  final VoiceState voiceState;

  const _ParticipantList({required this.voiceState});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    return ListView(
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
    );
  }
}

// ─── Control Bar ─────────────────────────────────────────────────────────────

class _ControlBar extends ConsumerWidget {
  final VoiceState voiceState;
  final VideoState videoState;

  const _ControlBar({required this.voiceState, required this.videoState});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    return Container(
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
            onTap: () => ref.read(voiceProvider.notifier).toggleMute(),
          ),
          const SizedBox(width: 16),
          _ControlButton(
            icon: voiceState.selfDeaf ? Icons.headset_off : Icons.headset,
            label: voiceState.selfDeaf ? 'Undeafen' : 'Deafen',
            isActive: voiceState.selfDeaf,
            onTap: () => ref.read(voiceProvider.notifier).toggleDeaf(),
          ),
          const SizedBox(width: 16),
          _ControlButton(
            icon: videoState.cameraEnabled ? Icons.videocam : Icons.videocam_off,
            label: videoState.cameraEnabled ? 'Cam Off' : 'Camera',
            isActive: videoState.cameraEnabled,
            activeColor: HavenTheme.online,
            onTap: () => ref.read(videoProvider.notifier).toggleCamera(),
          ),
          const SizedBox(width: 16),
          _ControlButton(
            icon: videoState.screenShareEnabled ? Icons.stop_screen_share : Icons.screen_share,
            label: videoState.screenShareEnabled ? 'Stop Share' : 'Share',
            isActive: videoState.screenShareEnabled,
            activeColor: HavenTheme.online,
            onTap: () => ref.read(videoProvider.notifier).toggleScreenShare(),
          ),

          // Screen audio volume (visible when any screen share is active)
          if (videoState.screenShareEnabled || videoState.remoteStreams.isNotEmpty) ...[
            const SizedBox(width: 16),
            _ScreenAudioVolumeButton(),
          ],

          // Eye toggle (own screen preview)
          if (videoState.screenShareEnabled) ...[
            const SizedBox(width: 16),
            _ControlButton(
              icon: videoState.showOwnScreen ? Icons.visibility : Icons.visibility_off,
              label: videoState.showOwnScreen ? 'Hide Self' : 'Show Self',
              isActive: videoState.showOwnScreen,
              activeColor: HavenTheme.primaryLight,
              onTap: () => ref.read(videoProvider.notifier).toggleShowOwnScreen(),
            ),
          ],

          // Collapse button
          if (videoState.hasAnyVideo) ...[
            const SizedBox(width: 16),
            _ControlButton(
              icon: videoState.videoMode == VideoMode.collapsed
                  ? Icons.open_in_full
                  : Icons.picture_in_picture_alt,
              label: videoState.videoMode == VideoMode.collapsed ? 'Expand' : 'Collapse',
              isActive: false,
              onTap: () {
                final notifier = ref.read(videoProvider.notifier);
                if (videoState.videoMode == VideoMode.collapsed) {
                  notifier.setVideoMode(VideoMode.expanded);
                } else {
                  notifier.setVideoMode(VideoMode.collapsed);
                }
              },
            ),
          ],

          // Fullscreen button
          if (videoState.hasAnyVideo && videoState.videoMode != VideoMode.collapsed) ...[
            const SizedBox(width: 16),
            _ControlButton(
              icon: Icons.fullscreen,
              label: 'Fullscreen',
              isActive: false,
              onTap: () => ref.read(videoProvider.notifier).setVideoMode(VideoMode.fullscreen),
            ),
          ],

          const SizedBox(width: 16),
          _ControlButton(
            icon: Icons.call_end,
            label: 'Leave',
            isActive: true,
            activeColor: HavenTheme.error,
            onTap: () => ref.read(voiceProvider.notifier).leaveVoice(),
          ),
        ],
      ),
    );
  }
}

// ─── Shared widgets ──────────────────────────────────────────────────────────

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

class _ScreenAudioVolumeButton extends ConsumerWidget {
  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final audioState = ref.watch(audioSettingsProvider);
    final effectiveVol = audioState.screenAudioMuted ? 0.0 : audioState.screenAudioVolume;

    IconData icon;
    if (effectiveVol == 0) {
      icon = Icons.volume_off;
    } else if (effectiveVol < 1.0) {
      icon = Icons.volume_down;
    } else {
      icon = Icons.volume_up;
    }

    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        PopupMenuButton<void>(
          tooltip: 'Screen audio ${(effectiveVol * 100).round()}%',
          padding: EdgeInsets.zero,
          constraints: const BoxConstraints(minWidth: 180),
          color: HavenTheme.surface,
          child: Material(
            color: audioState.screenAudioMuted
                ? HavenTheme.error.withValues(alpha: 0.2)
                : HavenTheme.surface,
            shape: const CircleBorder(),
            child: Padding(
              padding: const EdgeInsets.all(16),
              child: Icon(
                icon,
                color: audioState.screenAudioMuted
                    ? HavenTheme.error
                    : HavenTheme.textSecondary,
                size: 24,
              ),
            ),
          ),
          itemBuilder: (_) => [
            PopupMenuItem<void>(
              enabled: false,
              child: _ScreenAudioSliderContent(
                volume: audioState.screenAudioVolume,
                muted: audioState.screenAudioMuted,
              ),
            ),
          ],
        ),
        const SizedBox(height: 4),
        Text(
          'Screen Vol',
          style: TextStyle(fontSize: 11, color: HavenTheme.textMuted),
        ),
      ],
    );
  }
}

class _ScreenAudioSliderContent extends ConsumerStatefulWidget {
  final double volume;
  final bool muted;

  const _ScreenAudioSliderContent({required this.volume, required this.muted});

  @override
  ConsumerState<_ScreenAudioSliderContent> createState() =>
      _ScreenAudioSliderContentState();
}

class _ScreenAudioSliderContentState
    extends ConsumerState<_ScreenAudioSliderContent> {
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
              widget.muted ? 'Muted' : '${(_vol * 100).round()}%',
              style: const TextStyle(
                fontSize: 13,
                color: HavenTheme.textPrimary,
                fontWeight: FontWeight.w500,
              ),
            ),
            const Spacer(),
            InkWell(
              onTap: () {
                ref.read(audioSettingsProvider.notifier).toggleScreenAudioMute();
              },
              child: Padding(
                padding: const EdgeInsets.all(4),
                child: Icon(
                  widget.muted ? Icons.volume_up : Icons.volume_off,
                  size: 16,
                  color: widget.muted ? HavenTheme.online : HavenTheme.error,
                ),
              ),
            ),
            const SizedBox(width: 8),
            InkWell(
              onTap: () {
                setState(() => _vol = 1.0);
                ref.read(audioSettingsProvider.notifier).setScreenAudioVolume(1.0);
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
              ref.read(audioSettingsProvider.notifier).setScreenAudioVolume(v);
            },
          ),
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
