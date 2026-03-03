import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_webrtc/flutter_webrtc.dart';

import 'package:haven_app/config/theme.dart';
import 'package:haven_app/providers/auth_provider.dart';
import 'package:haven_app/providers/video_provider.dart';
import 'package:haven_app/providers/voice_provider.dart';
import 'package:haven_app/services/webrtc_service.dart';

class VideoPanel extends ConsumerStatefulWidget {
  const VideoPanel({super.key});

  @override
  ConsumerState<VideoPanel> createState() => _VideoPanelState();
}

class _VideoPanelState extends ConsumerState<VideoPanel> {
  Offset _position = const Offset(double.infinity, double.infinity);
  Size _size = const Size(640, 480);
  bool _initialized = false;

  static const _pipSize = Size(160, 120);
  static const _titleBarHeight = 32.0;
  static const _minSize = Size(320, 240);

  @override
  Widget build(BuildContext context) {
    final videoState = ref.watch(videoProvider);
    final voiceState = ref.watch(voiceProvider);
    final myUserId = ref.watch(authProvider).userId;
    final screenSize = MediaQuery.of(context).size;

    // Initialize position to bottom-right on first build
    if (!_initialized) {
      _initialized = true;
      if (videoState.panelMinimized) {
        _position = Offset(
          screenSize.width - _pipSize.width - 16,
          screenSize.height - _pipSize.height - 16,
        );
      } else {
        _position = Offset(
          screenSize.width - _size.width - 16,
          screenSize.height - _size.height - 16,
        );
      }
    }

    final isMinimized = videoState.panelMinimized;
    final currentSize = isMinimized ? _pipSize : _size;

    // Clamp position to screen bounds
    final clampedX = _position.dx.clamp(0.0, screenSize.width - currentSize.width);
    final clampedY = _position.dy.clamp(0.0, screenSize.height - currentSize.height);

    return Positioned(
      left: clampedX,
      top: clampedY,
      child: GestureDetector(
        onPanUpdate: (details) {
          setState(() {
            _position = Offset(
              clampedX + details.delta.dx,
              clampedY + details.delta.dy,
            );
          });
        },
        child: Material(
          elevation: 12,
          borderRadius: BorderRadius.circular(8),
          color: HavenTheme.surface,
          child: Container(
            width: currentSize.width,
            height: currentSize.height,
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(8),
              border: Border.all(color: HavenTheme.divider, width: 1),
            ),
            clipBehavior: Clip.antiAlias,
            child: Column(
              children: [
                // Title bar
                _buildTitleBar(isMinimized),
                // Video content
                Expanded(
                  child: isMinimized
                      ? _buildPipContent(videoState, myUserId)
                      : _buildExpandedContent(videoState, voiceState, myUserId),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }

  Widget _buildTitleBar(bool isMinimized) {
    return Container(
      height: _titleBarHeight,
      padding: const EdgeInsets.symmetric(horizontal: 8),
      decoration: const BoxDecoration(
        color: HavenTheme.sidebarBackground,
        border: Border(bottom: BorderSide(color: HavenTheme.divider)),
      ),
      child: Row(
        children: [
          const Icon(Icons.videocam, size: 14, color: HavenTheme.textMuted),
          const SizedBox(width: 6),
          const Text(
            'Video',
            style: TextStyle(
              fontSize: 12,
              color: HavenTheme.textSecondary,
              fontWeight: FontWeight.w500,
            ),
          ),
          const Spacer(),
          _titleButton(
            isMinimized ? Icons.open_in_full : Icons.close_fullscreen,
            () => ref.read(videoProvider.notifier).togglePanelMinimized(),
          ),
          _titleButton(
            Icons.close,
            () => ref.read(videoProvider.notifier).hidePanel(),
          ),
        ],
      ),
    );
  }

  Widget _titleButton(IconData icon, VoidCallback onTap) {
    return InkWell(
      onTap: onTap,
      borderRadius: BorderRadius.circular(4),
      child: Padding(
        padding: const EdgeInsets.all(4),
        child: Icon(icon, size: 14, color: HavenTheme.textMuted),
      ),
    );
  }

  /// PiP mode: show first screen share, or first camera, or first remote stream.
  Widget _buildPipContent(VideoState videoState, String? myUserId) {
    // Priority: any screen share > any camera
    // Check remote screen shares first
    for (final peerStreams in videoState.remoteStreams.values) {
      for (final rs in peerStreams) {
        if (rs.kind == VideoTrackKind.screen) {
          return _videoTile(rs.renderer, rs.peerId, isScreen: true);
        }
      }
    }
    // Local screen share
    if (videoState.localScreenRenderer != null) {
      return _videoTile(videoState.localScreenRenderer!, null, label: 'You', isScreen: true);
    }
    // Remote cameras
    for (final peerStreams in videoState.remoteStreams.values) {
      for (final rs in peerStreams) {
        if (rs.kind == VideoTrackKind.camera) {
          return _videoTile(rs.renderer, rs.peerId);
        }
      }
    }
    // Local camera
    if (videoState.localCameraRenderer != null) {
      return _videoTile(videoState.localCameraRenderer!, null, label: 'You', mirror: true);
    }

    return const Center(
      child: Text('No video', style: TextStyle(color: HavenTheme.textMuted, fontSize: 11)),
    );
  }

  /// Expanded mode: screen share gets ~70% area, cameras in row below.
  Widget _buildExpandedContent(VideoState videoState, VoiceState voiceState, String? myUserId) {
    // Collect all video tiles
    final List<Widget> screenTiles = [];
    final List<Widget> cameraTiles = [];

    // Local screen
    if (videoState.localScreenRenderer != null) {
      screenTiles.add(_videoTile(videoState.localScreenRenderer!, null, label: 'You (Screen)', isScreen: true));
    }

    // Local camera
    if (videoState.localCameraRenderer != null) {
      cameraTiles.add(_videoTile(videoState.localCameraRenderer!, null, label: 'You', mirror: true));
    }

    // Remote streams
    for (final entry in videoState.remoteStreams.entries) {
      final peerId = entry.key;
      for (final rs in entry.value) {
        final participant = voiceState.participants[peerId];
        final name = participant?.username ?? peerId.substring(0, 8);
        if (rs.kind == VideoTrackKind.screen) {
          screenTiles.add(_videoTile(rs.renderer, peerId, label: '$name (Screen)', isScreen: true));
        } else {
          cameraTiles.add(_videoTile(rs.renderer, peerId, label: name));
        }
      }
    }

    // Layout: if screen shares exist, they get top ~70%, cameras in bottom row.
    // If camera-only, use adaptive grid.
    if (screenTiles.isNotEmpty) {
      return Column(
        children: [
          // Screen shares area (70%)
          Expanded(
            flex: 7,
            child: screenTiles.length == 1
                ? screenTiles.first
                : Row(
                    children: screenTiles
                        .map((t) => Expanded(child: t))
                        .toList(),
                  ),
          ),
          // Camera row (30%)
          if (cameraTiles.isNotEmpty)
            SizedBox(
              height: 120,
              child: Row(
                children: cameraTiles
                    .map((t) => Expanded(child: t))
                    .toList(),
              ),
            ),
        ],
      );
    }

    // Camera-only: adaptive grid
    final allTiles = cameraTiles;
    if (allTiles.isEmpty) {
      return const Center(
        child: Text('No video', style: TextStyle(color: HavenTheme.textMuted)),
      );
    }

    return _buildAdaptiveGrid(allTiles);
  }

  Widget _buildAdaptiveGrid(List<Widget> tiles) {
    final count = tiles.length;
    if (count == 1) return tiles.first;
    if (count == 2) {
      return Row(children: tiles.map((t) => Expanded(child: t)).toList());
    }
    // 3-4: 2x2 grid
    if (count <= 4) {
      return Column(
        children: [
          Expanded(
            child: Row(
              children: tiles
                  .take(2)
                  .map((t) => Expanded(child: t))
                  .toList(),
            ),
          ),
          Expanded(
            child: Row(
              children: tiles
                  .skip(2)
                  .map((t) => Expanded(child: t))
                  .toList(),
            ),
          ),
        ],
      );
    }
    // 5-6: 3x2 grid
    return Column(
      children: [
        Expanded(
          child: Row(
            children: tiles
                .take(3)
                .map((t) => Expanded(child: t))
                .toList(),
          ),
        ),
        Expanded(
          child: Row(
            children: tiles
                .skip(3)
                .map((t) => Expanded(child: t))
                .toList(),
          ),
        ),
      ],
    );
  }

  Widget _videoTile(
    RTCVideoRenderer renderer,
    String? peerId, {
    String? label,
    bool mirror = false,
    bool isScreen = false,
  }) {
    final voiceState = ref.read(voiceProvider);
    final displayName = label ??
        (peerId != null
            ? (voiceState.participants[peerId]?.username ?? peerId.substring(0, 8))
            : 'Unknown');

    return Container(
      color: Colors.black,
      child: Stack(
        fit: StackFit.expand,
        children: [
          RTCVideoView(
            renderer,
            mirror: mirror,
            objectFit: isScreen
                ? RTCVideoViewObjectFit.RTCVideoViewObjectFitContain
                : RTCVideoViewObjectFit.RTCVideoViewObjectFitCover,
          ),
          // Username label
          Positioned(
            left: 4,
            bottom: 4,
            child: Container(
              padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 2),
              decoration: BoxDecoration(
                color: Colors.black54,
                borderRadius: BorderRadius.circular(4),
              ),
              child: Text(
                displayName,
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
    );
  }
}
