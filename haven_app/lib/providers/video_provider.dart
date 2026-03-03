import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_webrtc/flutter_webrtc.dart';

import 'package:haven_app/config/theme.dart';
import 'package:haven_app/providers/auth_provider.dart';
import 'package:haven_app/providers/gateway_provider.dart';
import 'package:haven_app/services/webrtc_service.dart';

class RemoteStream {
  final String peerId;
  final VideoTrackKind kind;
  final MediaStream stream;
  final RTCVideoRenderer renderer;

  RemoteStream({
    required this.peerId,
    required this.kind,
    required this.stream,
    required this.renderer,
  });
}

class VideoState {
  final bool cameraEnabled;
  final bool screenShareEnabled;
  final MediaStream? localCameraStream;
  final MediaStream? localScreenStream;
  final RTCVideoRenderer? localCameraRenderer;
  final RTCVideoRenderer? localScreenRenderer;
  final Map<String, List<RemoteStream>> remoteStreams;
  final bool panelVisible;
  final bool panelMinimized;

  const VideoState({
    this.cameraEnabled = false,
    this.screenShareEnabled = false,
    this.localCameraStream,
    this.localScreenStream,
    this.localCameraRenderer,
    this.localScreenRenderer,
    this.remoteStreams = const {},
    this.panelVisible = false,
    this.panelMinimized = false,
  });

  VideoState copyWith({
    bool? cameraEnabled,
    bool? screenShareEnabled,
    MediaStream? localCameraStream,
    bool clearLocalCamera = false,
    MediaStream? localScreenStream,
    bool clearLocalScreen = false,
    RTCVideoRenderer? localCameraRenderer,
    bool clearLocalCameraRenderer = false,
    RTCVideoRenderer? localScreenRenderer,
    bool clearLocalScreenRenderer = false,
    Map<String, List<RemoteStream>>? remoteStreams,
    bool? panelVisible,
    bool? panelMinimized,
  }) {
    return VideoState(
      cameraEnabled: cameraEnabled ?? this.cameraEnabled,
      screenShareEnabled: screenShareEnabled ?? this.screenShareEnabled,
      localCameraStream: clearLocalCamera ? null : (localCameraStream ?? this.localCameraStream),
      localScreenStream: clearLocalScreen ? null : (localScreenStream ?? this.localScreenStream),
      localCameraRenderer: clearLocalCameraRenderer ? null : (localCameraRenderer ?? this.localCameraRenderer),
      localScreenRenderer: clearLocalScreenRenderer ? null : (localScreenRenderer ?? this.localScreenRenderer),
      remoteStreams: remoteStreams ?? this.remoteStreams,
      panelVisible: panelVisible ?? this.panelVisible,
      panelMinimized: panelMinimized ?? this.panelMinimized,
    );
  }

  bool get hasAnyVideo =>
      cameraEnabled ||
      screenShareEnabled ||
      remoteStreams.values.any((list) => list.isNotEmpty);
}

class VideoNotifier extends StateNotifier<VideoState> {
  final Ref _ref;
  WebRTCService? _webrtcService;

  /// Must be set to show the screen source picker dialog.
  BuildContext? overlayContext;

  VideoNotifier(this._ref) : super(const VideoState());

  /// Initialize WebRTC service after joining voice.
  void initWebRTC() {
    final gateway = _ref.read(gatewayServiceProvider);
    final authState = _ref.read(authProvider);
    final userId = authState.userId;
    if (userId == null) return;

    _webrtcService = WebRTCService(
      gateway: gateway,
      localUserId: userId,
    );

    _webrtcService!.onRemoteTrackAdded = _onRemoteTrackAdded;
    _webrtcService!.onRemoteTrackRemoved = _onRemoteTrackRemoved;
    _webrtcService!.onPickScreenSource = _pickScreenSource;
  }

  /// Handle an incoming VoiceSignal event from the gateway.
  Future<void> handleSignal(String fromUserId, Map<String, dynamic> signal) async {
    await _webrtcService?.handleSignal(fromUserId, signal);
  }

  /// A peer joined voice — establish WebRTC connection.
  Future<void> handlePeerJoined(String peerId) async {
    await _webrtcService?.connectToPeer(peerId);
  }

  /// A peer left voice — clean up their connection and streams.
  Future<void> handlePeerLeft(String peerId) async {
    await _webrtcService?.removePeer(peerId);

    final updated = Map<String, List<RemoteStream>>.from(state.remoteStreams);
    final streams = updated.remove(peerId);
    if (streams != null) {
      for (final rs in streams) {
        rs.renderer.srcObject = null;
        await rs.renderer.dispose();
      }
    }
    final newState = state.copyWith(remoteStreams: updated);
    state = newState.copyWith(panelVisible: newState.hasAnyVideo);
  }

  /// Toggle camera on/off.
  Future<void> toggleCamera() async {
    if (_webrtcService == null) return;

    if (state.cameraEnabled) {
      await _webrtcService!.stopCamera();
      final renderer = state.localCameraRenderer;
      if (renderer != null) {
        renderer.srcObject = null;
        await renderer.dispose();
      }
      state = state.copyWith(
        cameraEnabled: false,
        clearLocalCamera: true,
        clearLocalCameraRenderer: true,
      );
      state = state.copyWith(panelVisible: state.hasAnyVideo);
    } else {
      final stream = await _webrtcService!.startCamera();
      if (stream != null) {
        final renderer = RTCVideoRenderer();
        await renderer.initialize();
        renderer.srcObject = stream;
        state = state.copyWith(
          cameraEnabled: true,
          localCameraStream: stream,
          localCameraRenderer: renderer,
          panelVisible: true,
        );
      }
    }
  }

  /// Toggle screen share on/off.
  Future<void> toggleScreenShare() async {
    if (_webrtcService == null) return;

    if (state.screenShareEnabled) {
      await _webrtcService!.stopScreenShare();
      final renderer = state.localScreenRenderer;
      if (renderer != null) {
        renderer.srcObject = null;
        await renderer.dispose();
      }
      state = state.copyWith(
        screenShareEnabled: false,
        clearLocalScreen: true,
        clearLocalScreenRenderer: true,
      );
      state = state.copyWith(panelVisible: state.hasAnyVideo);
    } else {
      final stream = await _webrtcService!.startScreenShare();
      if (stream != null) {
        final renderer = RTCVideoRenderer();
        await renderer.initialize();
        renderer.srcObject = stream;
        state = state.copyWith(
          screenShareEnabled: true,
          localScreenStream: stream,
          localScreenRenderer: renderer,
          panelVisible: true,
        );
      }
    }
  }

  void togglePanelMinimized() {
    state = state.copyWith(panelMinimized: !state.panelMinimized);
  }

  void hidePanel() {
    state = state.copyWith(panelVisible: false);
  }

  void showPanel() {
    if (state.hasAnyVideo) {
      state = state.copyWith(panelVisible: true);
    }
  }

  /// Dispose all WebRTC resources.
  Future<void> disposeWebRTC() async {
    // Dispose local renderers
    final camRenderer = state.localCameraRenderer;
    if (camRenderer != null) {
      camRenderer.srcObject = null;
      await camRenderer.dispose();
    }
    final screenRenderer = state.localScreenRenderer;
    if (screenRenderer != null) {
      screenRenderer.srcObject = null;
      await screenRenderer.dispose();
    }

    // Dispose remote renderers
    for (final streams in state.remoteStreams.values) {
      for (final rs in streams) {
        rs.renderer.srcObject = null;
        await rs.renderer.dispose();
      }
    }

    await _webrtcService?.dispose();
    _webrtcService = null;
    state = const VideoState();
  }

  // -- Private callbacks --

  Future<DesktopCapturerSource?> _pickScreenSource(
      List<DesktopCapturerSource> sources) async {
    final ctx = overlayContext;
    if (ctx == null || !ctx.mounted) {
      // No context — fall back to first screen
      return sources.firstWhere(
        (s) => s.type == SourceType.Screen,
        orElse: () => sources.first,
      );
    }

    return showDialog<DesktopCapturerSource>(
      context: ctx,
      builder: (context) => _ScreenSourcePickerDialog(sources: sources),
    );
  }

  Future<void> _onRemoteTrackAdded(RemoteVideoTrack track) async {
    final renderer = RTCVideoRenderer();
    await renderer.initialize();
    renderer.srcObject = track.stream;

    final rs = RemoteStream(
      peerId: track.peerId,
      kind: track.kind,
      stream: track.stream,
      renderer: renderer,
    );

    final updated = Map<String, List<RemoteStream>>.from(state.remoteStreams);
    updated.putIfAbsent(track.peerId, () => []).add(rs);
    state = state.copyWith(
      remoteStreams: updated,
      panelVisible: true,
    );
  }

  Future<void> _onRemoteTrackRemoved(String peerId, String streamId) async {
    final updated = Map<String, List<RemoteStream>>.from(state.remoteStreams);
    final peerStreams = updated[peerId];
    if (peerStreams != null) {
      final toRemove = peerStreams.where((rs) => rs.stream.id == streamId).toList();
      for (final rs in toRemove) {
        rs.renderer.srcObject = null;
        await rs.renderer.dispose();
      }
      peerStreams.removeWhere((rs) => rs.stream.id == streamId);
      if (peerStreams.isEmpty) {
        updated.remove(peerId);
      }
    }
    final newState = state.copyWith(remoteStreams: updated);
    state = newState.copyWith(panelVisible: newState.hasAnyVideo);
  }
}

final videoProvider =
    StateNotifierProvider<VideoNotifier, VideoState>((ref) {
  return VideoNotifier(ref);
});

class _ScreenSourcePickerDialog extends StatelessWidget {
  final List<DesktopCapturerSource> sources;

  const _ScreenSourcePickerDialog({required this.sources});

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
                  children: screens.map((s) => _SourceTile(
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
                    itemBuilder: (context, i) => _SourceTile(
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

class _SourceTile extends StatelessWidget {
  final DesktopCapturerSource source;
  final VoidCallback onTap;

  const _SourceTile({required this.source, required this.onTap});

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
