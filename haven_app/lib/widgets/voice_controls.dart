import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_webrtc/flutter_webrtc.dart';

import 'package:haven_app/config/theme.dart';
import 'package:haven_app/providers/audio_settings_provider.dart';
import 'package:haven_app/providers/video_provider.dart';
import 'package:haven_app/providers/voice_provider.dart';
import 'package:haven_app/services/webrtc_service.dart';

class VoiceControls extends ConsumerWidget {
  const VoiceControls({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final voiceState = ref.watch(voiceProvider);
    final videoState = ref.watch(videoProvider);

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

class _VideoGrid extends ConsumerWidget {
  final VideoState videoState;
  final VoiceState voiceState;

  const _VideoGrid({required this.videoState, required this.voiceState});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final tiles = _collectTiles();
    if (tiles.isEmpty) {
      return const Center(
        child: Text('No video', style: TextStyle(color: HavenTheme.textMuted)),
      );
    }

    final focusedId = videoState.focusedStreamId;
    if (focusedId != null) {
      final focusedIdx = tiles.indexWhere((t) => t.id == focusedId);
      if (focusedIdx >= 0) {
        return _buildFocusedLayout(tiles, focusedIdx, ref);
      }
    }

    return _buildEqualGrid(tiles, ref);
  }

  List<_VideoTileData> _collectTiles() {
    final tiles = <_VideoTileData>[];

    // Local camera
    if (videoState.cameraEnabled && videoState.localCameraRenderer != null) {
      tiles.add(_VideoTileData(
        id: 'local-camera',
        renderer: videoState.localCameraRenderer!,
        label: 'You',
        mirror: true,
        isScreen: false,
      ));
    }

    // Local screen (only if showOwnScreen)
    if (videoState.screenShareEnabled && videoState.showOwnScreen && videoState.localScreenRenderer != null) {
      tiles.add(_VideoTileData(
        id: 'local-screen',
        renderer: videoState.localScreenRenderer!,
        label: 'You (Screen)',
        isScreen: true,
      ));
    }

    // Remote streams
    for (final entry in videoState.remoteStreams.entries) {
      final peerId = entry.key;
      for (final rs in entry.value) {
        final participant = voiceState.participants[peerId];
        final name = participant?.username ?? peerId.substring(0, 8);
        tiles.add(_VideoTileData(
          id: '$peerId-${rs.stream.id}',
          renderer: rs.renderer,
          label: rs.kind == VideoTrackKind.screen ? '$name (Screen)' : name,
          isScreen: rs.kind == VideoTrackKind.screen,
        ));
      }
    }

    return tiles;
  }

  Widget _buildEqualGrid(List<_VideoTileData> tiles, WidgetRef ref) {
    final count = tiles.length;
    if (count == 1) return _tile(tiles[0], ref);
    if (count == 2) {
      return Row(
        children: tiles.map((t) => Expanded(child: _tile(t, ref))).toList(),
      );
    }
    if (count <= 4) {
      return Column(
        children: [
          Expanded(
            child: Row(
              children: tiles.take(2).map((t) => Expanded(child: _tile(t, ref))).toList(),
            ),
          ),
          Expanded(
            child: Row(
              children: tiles.skip(2).map((t) => Expanded(child: _tile(t, ref))).toList(),
            ),
          ),
        ],
      );
    }
    // 5-6: 3x2
    return Column(
      children: [
        Expanded(
          child: Row(
            children: tiles.take(3).map((t) => Expanded(child: _tile(t, ref))).toList(),
          ),
        ),
        Expanded(
          child: Row(
            children: tiles.skip(3).map((t) => Expanded(child: _tile(t, ref))).toList(),
          ),
        ),
      ],
    );
  }

  Widget _buildFocusedLayout(List<_VideoTileData> tiles, int focusedIdx, WidgetRef ref) {
    final focused = tiles[focusedIdx];
    final others = [...tiles]..removeAt(focusedIdx);

    return Row(
      children: [
        Expanded(
          flex: 3,
          child: _tile(focused, ref),
        ),
        if (others.isNotEmpty)
          SizedBox(
            width: 180,
            child: ListView(
              children: others.map((t) => SizedBox(
                height: 120,
                child: _tile(t, ref),
              )).toList(),
            ),
          ),
      ],
    );
  }

  Widget _tile(_VideoTileData data, WidgetRef ref) {
    return GestureDetector(
      onTap: () => ref.read(videoProvider.notifier).setFocusedStream(data.id),
      child: Container(
        margin: const EdgeInsets.all(1),
        color: Colors.black,
        child: Stack(
          fit: StackFit.expand,
          children: [
            RTCVideoView(
              data.renderer,
              mirror: data.mirror,
              objectFit: data.isScreen
                  ? RTCVideoViewObjectFit.RTCVideoViewObjectFitContain
                  : RTCVideoViewObjectFit.RTCVideoViewObjectFitCover,
            ),
            Positioned(
              left: 6,
              bottom: 6,
              child: Container(
                padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 2),
                decoration: BoxDecoration(
                  color: Colors.black54,
                  borderRadius: BorderRadius.circular(4),
                ),
                child: Text(
                  data.label,
                  style: const TextStyle(
                    color: Colors.white,
                    fontSize: 11,
                    fontWeight: FontWeight.w500,
                  ),
                ),
              ),
            ),
          ],
        ),
      ),
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

class _VideoTileData {
  final String id;
  final RTCVideoRenderer renderer;
  final String label;
  final bool mirror;
  final bool isScreen;

  _VideoTileData({
    required this.id,
    required this.renderer,
    required this.label,
    this.mirror = false,
    this.isScreen = false,
  });
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
