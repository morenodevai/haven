import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_webrtc/flutter_webrtc.dart';

import 'package:haven_app/providers/auth_provider.dart';
import 'package:haven_app/providers/gateway_provider.dart';
import 'package:haven_app/services/webrtc_service.dart';
import 'package:haven_app/widgets/screen_source_picker.dart';

enum VideoMode { collapsed, expanded, fullscreen }

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
  final VideoMode videoMode;
  final bool showOwnScreen;
  final String? focusedStreamId;

  const VideoState({
    this.cameraEnabled = false,
    this.screenShareEnabled = false,
    this.localCameraStream,
    this.localScreenStream,
    this.localCameraRenderer,
    this.localScreenRenderer,
    this.remoteStreams = const {},
    this.videoMode = VideoMode.expanded,
    this.showOwnScreen = false,
    this.focusedStreamId,
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
    VideoMode? videoMode,
    bool? showOwnScreen,
    String? focusedStreamId,
    bool clearFocusedStream = false,
  }) {
    return VideoState(
      cameraEnabled: cameraEnabled ?? this.cameraEnabled,
      screenShareEnabled: screenShareEnabled ?? this.screenShareEnabled,
      localCameraStream: clearLocalCamera ? null : (localCameraStream ?? this.localCameraStream),
      localScreenStream: clearLocalScreen ? null : (localScreenStream ?? this.localScreenStream),
      localCameraRenderer: clearLocalCameraRenderer ? null : (localCameraRenderer ?? this.localCameraRenderer),
      localScreenRenderer: clearLocalScreenRenderer ? null : (localScreenRenderer ?? this.localScreenRenderer),
      remoteStreams: remoteStreams ?? this.remoteStreams,
      videoMode: videoMode ?? this.videoMode,
      showOwnScreen: showOwnScreen ?? this.showOwnScreen,
      focusedStreamId: clearFocusedStream ? null : (focusedStreamId ?? this.focusedStreamId),
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

  void setVideoMode(VideoMode mode) {
    state = state.copyWith(videoMode: mode);
  }

  void toggleShowOwnScreen() {
    state = state.copyWith(showOwnScreen: !state.showOwnScreen);
  }

  void setFocusedStream(String? id) {
    if (id == state.focusedStreamId) {
      // Click focused again → clear
      state = state.copyWith(clearFocusedStream: true);
    } else {
      state = state.copyWith(focusedStreamId: id, clearFocusedStream: false);
    }
  }

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
    state = state.copyWith(remoteStreams: updated);
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
          videoMode: VideoMode.expanded,
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
    } else {
      try {
        final stream = await _webrtcService!.startScreenShare();
        if (stream != null) {
          final renderer = RTCVideoRenderer();
          await renderer.initialize();
          renderer.srcObject = stream;
          state = state.copyWith(
            screenShareEnabled: true,
            localScreenStream: stream,
            localScreenRenderer: renderer,
            videoMode: VideoMode.expanded,
          );
        }
      } catch (e) {
        debugPrint('Screen share failed: $e');
      }
    }
  }

  /// Dispose all WebRTC resources.
  Future<void> disposeWebRTC() async {
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
      return sources.firstWhere(
        (s) => s.type == SourceType.Screen,
        orElse: () => sources.first,
      );
    }

    return showDialog<DesktopCapturerSource>(
      context: ctx,
      builder: (context) => ScreenSourcePickerDialog(sources: sources),
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
      videoMode: state.hasAnyVideo ? state.videoMode : VideoMode.expanded,
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
    state = state.copyWith(remoteStreams: updated);
  }
}

final videoProvider =
    StateNotifierProvider<VideoNotifier, VideoState>((ref) {
  return VideoNotifier(ref);
});
