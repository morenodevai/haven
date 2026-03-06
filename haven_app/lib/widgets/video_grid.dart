import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_webrtc/flutter_webrtc.dart';

import 'package:haven_app/providers/video_provider.dart';
import 'package:haven_app/providers/voice_provider.dart';
import 'package:haven_app/services/webrtc_service.dart';

/// Data for a single video tile in the grid.
class VideoTileData {
  final String id;
  final RTCVideoRenderer renderer;
  final String label;
  final bool mirror;
  final bool isScreen;

  VideoTileData({
    required this.id,
    required this.renderer,
    required this.label,
    this.mirror = false,
    this.isScreen = false,
  });
}

/// Collects all video tiles from the current video and voice state.
List<VideoTileData> collectVideoTiles(VideoState videoState, VoiceState voiceState) {
  final tiles = <VideoTileData>[];

  // Local camera
  if (videoState.cameraEnabled && videoState.localCameraRenderer != null) {
    tiles.add(VideoTileData(
      id: 'local-camera',
      renderer: videoState.localCameraRenderer!,
      label: 'You',
      mirror: true,
      isScreen: false,
    ));
  }

  // Local screen (only if showOwnScreen)
  if (videoState.screenShareEnabled && videoState.showOwnScreen && videoState.localScreenRenderer != null) {
    tiles.add(VideoTileData(
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
      tiles.add(VideoTileData(
        id: '$peerId-${rs.stream.id}',
        renderer: rs.renderer,
        label: rs.kind == VideoTrackKind.screen ? '$name (Screen)' : name,
        isScreen: rs.kind == VideoTrackKind.screen,
      ));
    }
  }

  return tiles;
}

/// Builds a video grid layout from a list of tiles.
///
/// Shows an equal grid by default, or a focused layout if [focusedStreamId]
/// matches one of the tiles. [labelColor] controls the tile label text color.
class VideoGrid extends ConsumerWidget {
  final List<VideoTileData> tiles;
  final String? focusedStreamId;
  final Color labelColor;
  final String emptyText;

  const VideoGrid({
    super.key,
    required this.tiles,
    this.focusedStreamId,
    this.labelColor = Colors.white,
    this.emptyText = 'No video',
  });

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    if (tiles.isEmpty) {
      return Center(
        child: Text(emptyText, style: TextStyle(color: labelColor.withValues(alpha: 0.5))),
      );
    }

    if (focusedStreamId != null) {
      final focusedIdx = tiles.indexWhere((t) => t.id == focusedStreamId);
      if (focusedIdx >= 0) {
        return _buildFocusedLayout(tiles, focusedIdx, ref);
      }
    }

    return _buildEqualGrid(tiles, ref);
  }

  Widget _buildEqualGrid(List<VideoTileData> tiles, WidgetRef ref) {
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

  Widget _buildFocusedLayout(List<VideoTileData> tiles, int focusedIdx, WidgetRef ref) {
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

  Widget _tile(VideoTileData data, WidgetRef ref) {
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
                  style: TextStyle(
                    color: labelColor,
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
