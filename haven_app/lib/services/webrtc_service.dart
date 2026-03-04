import 'dart:async';

import 'package:flutter_webrtc/flutter_webrtc.dart';

import 'package:haven_app/services/gateway_service.dart';

/// Track type sent via TrackInfo before SDP offers.
enum VideoTrackKind { camera, screen }

/// A remote video track with metadata.
class RemoteVideoTrack {
  final String peerId;
  final VideoTrackKind kind;
  final MediaStream stream;

  RemoteVideoTrack({
    required this.peerId,
    required this.kind,
    required this.stream,
  });
}

/// Manages full-mesh WebRTC peer connections for video/screen share.
///
/// Audio is NOT carried over WebRTC — the existing WASAPI+relay path is used.
/// This service only handles video tracks (camera + screen share).
class WebRTCService {
  final GatewayService _gateway;
  final String _localUserId;

  final Map<String, RTCPeerConnection> _peers = {};
  final Map<String, Map<String, VideoTrackKind>> _pendingTrackInfo = {};

  MediaStream? _cameraStream;
  MediaStream? _screenStream;

  void Function(RemoteVideoTrack track)? onRemoteTrackAdded;
  void Function(String peerId, String streamId)? onRemoteTrackRemoved;

  static const _rtcConfig = {
    'iceServers': [
      {'urls': 'stun:stun.l.google.com:19302'},
    ],
  };

  WebRTCService({
    required GatewayService gateway,
    required String localUserId,
  })  : _gateway = gateway,
        _localUserId = localUserId;

  MediaStream? get cameraStream => _cameraStream;
  MediaStream? get screenStream => _screenStream;

  /// Connect to a set of peers. Alphabetical userId comparison determines
  /// who creates the offer to avoid duplicate offers.
  Future<void> connectToPeers(List<String> peerIds) async {
    for (final peerId in peerIds) {
      if (peerId == _localUserId || _peers.containsKey(peerId)) continue;
      await _createPeer(peerId);
      if (_localUserId.compareTo(peerId) < 0) {
        await _createAndSendOffer(peerId);
      }
    }
  }

  /// Connect to a single new peer.
  Future<void> connectToPeer(String peerId) async {
    if (peerId == _localUserId || _peers.containsKey(peerId)) return;
    await _createPeer(peerId);
    if (_localUserId.compareTo(peerId) < 0) {
      await _createAndSendOffer(peerId);
    }
  }

  /// Remove a peer (they left voice).
  Future<void> removePeer(String peerId) async {
    final pc = _peers.remove(peerId);
    await pc?.close();
    _pendingTrackInfo.remove(peerId);
  }

  /// Handle an incoming signaling message from the gateway.
  Future<void> handleSignal(String fromUserId, Map<String, dynamic> signal) async {
    final type = signal['signal_type'] as String?;
    if (type == null) return;

    switch (type) {
      case 'TrackInfo':
        final streamId = signal['stream_id'] as String;
        final kindStr = signal['track_type'] as String;
        final kind = kindStr == 'screen' ? VideoTrackKind.screen : VideoTrackKind.camera;
        _pendingTrackInfo.putIfAbsent(fromUserId, () => {})[streamId] = kind;
        break;

      case 'Offer':
        await _handleOffer(fromUserId, signal);
        break;

      case 'Answer':
        await _handleAnswer(fromUserId, signal);
        break;

      case 'IceCandidate':
        await _handleIceCandidate(fromUserId, signal);
        break;
    }
  }

  /// Start camera capture and add tracks to all peers.
  Future<MediaStream?> startCamera() async {
    if (_cameraStream != null) return _cameraStream;

    _cameraStream = await navigator.mediaDevices.getUserMedia({
      'video': {
        'width': {'ideal': 1280},
        'height': {'ideal': 720},
        'frameRate': {'ideal': 60},
      },
      'audio': false,
    });

    await _addStreamToPeers(_cameraStream!, VideoTrackKind.camera);
    return _cameraStream;
  }

  /// Stop camera capture and remove tracks from all peers.
  Future<void> stopCamera() async {
    if (_cameraStream == null) return;

    await _removeStreamFromPeers(_cameraStream!);
    for (final track in _cameraStream!.getTracks()) {
      await track.stop();
    }
    await _cameraStream!.dispose();
    _cameraStream = null;
  }

  /// Callback for showing the screen source picker UI.
  /// Must be set before calling startScreenShare.
  Future<DesktopCapturerSource?> Function(List<DesktopCapturerSource> sources)? onPickScreenSource;

  /// Start screen share and add tracks to all peers.
  /// Uses desktopCapturer to enumerate sources, then presents a picker.
  Future<MediaStream?> startScreenShare() async {
    if (_screenStream != null) return _screenStream;

    // Enumerate available screens and windows
    final sources = await desktopCapturer.getSources(
      types: [SourceType.Screen, SourceType.Window],
    );

    if (sources.isEmpty) return null;

    // Let user pick a source
    DesktopCapturerSource? selected;
    if (onPickScreenSource != null) {
      selected = await onPickScreenSource!(sources);
    } else if (sources.isNotEmpty) {
      // Fallback: pick entire screen (first Screen source)
      selected = sources.firstWhere(
        (s) => s.type == SourceType.Screen,
        orElse: () => sources.first,
      );
    }
    if (selected == null) return null;

    // Create stream from selected source using getDisplayMedia (not getUserMedia)
    _screenStream = await navigator.mediaDevices.getDisplayMedia(<String, dynamic>{
      'video': {
        'deviceId': {'exact': selected.id},
        'mandatory': {
          'frameRate': 60.0,
          'minWidth': 1280,
          'minHeight': 720,
          'maxWidth': 1920,
          'maxHeight': 1080,
        },
      },
    });

    if (_screenStream == null || _screenStream!.getVideoTracks().isEmpty) {
      _screenStream?.dispose();
      _screenStream = null;
      return null;
    }

    // Detect when user stops screen share via OS controls
    _screenStream!.getVideoTracks().first.onEnded = () {
      stopScreenShare();
    };

    await _addStreamToPeers(_screenStream!, VideoTrackKind.screen);
    return _screenStream;
  }

  /// Stop screen share and remove tracks from all peers.
  Future<void> stopScreenShare() async {
    if (_screenStream == null) return;

    await _removeStreamFromPeers(_screenStream!);
    for (final track in _screenStream!.getTracks()) {
      await track.stop();
    }
    await _screenStream!.dispose();
    _screenStream = null;
  }

  /// Dispose all connections and streams.
  Future<void> dispose() async {
    await stopCamera();
    await stopScreenShare();
    for (final pc in _peers.values) {
      await pc.close();
    }
    _peers.clear();
    _pendingTrackInfo.clear();
  }

  // -- Private helpers --

  Future<RTCPeerConnection> _createPeer(String peerId) async {
    final pc = await createPeerConnection(_rtcConfig);

    pc.onIceCandidate = (candidate) {
      _gateway.voiceSignalSend(peerId, {
        'signal_type': 'IceCandidate',
        'candidate': candidate.candidate,
        'sdp_mid': candidate.sdpMid,
        'sdp_m_line_index': candidate.sdpMLineIndex,
      });
    };

    pc.onTrack = (event) {
      if (event.streams.isNotEmpty) {
        final stream = event.streams.first;
        final trackInfoMap = _pendingTrackInfo[peerId];
        final kind = trackInfoMap?[stream.id] ?? VideoTrackKind.camera;

        onRemoteTrackAdded?.call(RemoteVideoTrack(
          peerId: peerId,
          kind: kind,
          stream: stream,
        ));
      }
    };

    pc.onRemoveStream = (stream) {
      onRemoteTrackRemoved?.call(peerId, stream.id);
    };

    // Add existing local streams
    if (_cameraStream != null) {
      _sendTrackInfo(peerId, _cameraStream!.id, VideoTrackKind.camera);
      for (final track in _cameraStream!.getTracks()) {
        final sender = await pc.addTrack(track, _cameraStream!);
        await _applyBitrate(sender, VideoTrackKind.camera);
      }
    }
    if (_screenStream != null) {
      _sendTrackInfo(peerId, _screenStream!.id, VideoTrackKind.screen);
      for (final track in _screenStream!.getTracks()) {
        final sender = await pc.addTrack(track, _screenStream!);
        await _applyBitrate(sender, VideoTrackKind.screen);
      }
    }

    _peers[peerId] = pc;
    return pc;
  }

  Future<void> _createAndSendOffer(String peerId) async {
    final pc = _peers[peerId];
    if (pc == null) return;

    final offer = await pc.createOffer();
    final mungedSdp = _setSdpBandwidth(offer.sdp!, _sdpBandwidthKbps);
    final mungedOffer = RTCSessionDescription(mungedSdp, offer.type);
    await pc.setLocalDescription(mungedOffer);

    _gateway.voiceSignalSend(peerId, {
      'signal_type': 'Offer',
      'sdp': mungedSdp,
    });
  }

  /// SDP bandwidth for video in kbps — 10 Mbps ceiling.
  static const _sdpBandwidthKbps = 10000;

  Future<void> _handleOffer(String fromUserId, Map<String, dynamic> signal) async {
    var pc = _peers[fromUserId];
    if (pc == null) {
      pc = await _createPeer(fromUserId);
    }

    // Glare handling: if we have a local offer, lower userId rolls back
    final signalingState = pc.signalingState;
    if (signalingState == RTCSignalingState.RTCSignalingStateHaveLocalOffer) {
      if (_localUserId.compareTo(fromUserId) > 0) {
        // We have higher userId — ignore their offer, they should accept ours
        return;
      }
      // We have lower userId — roll back our offer and accept theirs
      await pc.setLocalDescription(RTCSessionDescription('', 'rollback'));
    }

    await pc.setRemoteDescription(RTCSessionDescription(
      signal['sdp'] as String,
      'offer',
    ));

    final answer = await pc.createAnswer();
    final mungedSdp = _setSdpBandwidth(answer.sdp!, _sdpBandwidthKbps);
    final mungedAnswer = RTCSessionDescription(mungedSdp, answer.type);
    await pc.setLocalDescription(mungedAnswer);

    _gateway.voiceSignalSend(fromUserId, {
      'signal_type': 'Answer',
      'sdp': mungedSdp,
    });
  }

  Future<void> _handleAnswer(String fromUserId, Map<String, dynamic> signal) async {
    final pc = _peers[fromUserId];
    if (pc == null) return;

    await pc.setRemoteDescription(RTCSessionDescription(
      signal['sdp'] as String,
      'answer',
    ));
  }

  Future<void> _handleIceCandidate(String fromUserId, Map<String, dynamic> signal) async {
    final pc = _peers[fromUserId];
    if (pc == null) return;

    await pc.addCandidate(RTCIceCandidate(
      signal['candidate'] as String,
      signal['sdp_mid'] as String?,
      signal['sdp_m_line_index'] as int?,
    ));
  }

  void _sendTrackInfo(String peerId, String streamId, VideoTrackKind kind) {
    _gateway.voiceSignalSend(peerId, {
      'signal_type': 'TrackInfo',
      'stream_id': streamId,
      'track_type': kind == VideoTrackKind.screen ? 'screen' : 'camera',
    });
  }

  Future<void> _addStreamToPeers(MediaStream stream, VideoTrackKind kind) async {
    for (final entry in _peers.entries) {
      _sendTrackInfo(entry.key, stream.id, kind);
      for (final track in stream.getTracks()) {
        final sender = await entry.value.addTrack(track, stream);
        await _applyBitrate(sender, kind);
      }
      // Renegotiate
      await _createAndSendOffer(entry.key);
    }
  }

  /// Set encoding parameters on an RTP sender.
  /// Camera: 4 Mbps max, Screen share: 8 Mbps max.
  /// degradationPreference = maintain-resolution so encoder drops frames, not pixels.
  Future<void> _applyBitrate(RTCRtpSender sender, VideoTrackKind kind) async {
    final params = sender.parameters;
    final maxBitrate = kind == VideoTrackKind.screen ? 8000000 : 4000000;
    final minBitrate = kind == VideoTrackKind.screen ? 2000000 : 1000000;
    params.degradationPreference = RTCDegradationPreference.DISABLED;
    if (params.encodings == null || params.encodings!.isEmpty) {
      params.encodings = [RTCRtpEncoding(
        maxBitrate: maxBitrate,
        minBitrate: minBitrate,
      )];
    } else {
      for (final encoding in params.encodings!) {
        encoding.maxBitrate = maxBitrate;
        encoding.minBitrate = minBitrate;
      }
    }
    await sender.setParameters(params);
  }

  /// Munge SDP to set video bitrate via b=AS line (kbps).
  String _setSdpBandwidth(String sdp, int kbps) {
    final lines = sdp.split('\r\n');
    final result = <String>[];
    for (var i = 0; i < lines.length; i++) {
      result.add(lines[i]);
      // After m=video line, insert bandwidth
      if (lines[i].startsWith('m=video')) {
        // Remove any existing b= lines
        while (i + 1 < lines.length && lines[i + 1].startsWith('b=')) {
          i++;
        }
        result.add('b=AS:$kbps');
      }
    }
    return result.join('\r\n');
  }

  Future<void> _removeStreamFromPeers(MediaStream stream) async {
    for (final pc in _peers.values) {
      final senders = await pc.getSenders();
      for (final sender in senders) {
        if (sender.track != null &&
            stream.getTracks().any((t) => t.id == sender.track!.id)) {
          await pc.removeTrack(sender);
        }
      }
    }
    // Renegotiate all peers
    for (final peerId in _peers.keys) {
      await _createAndSendOffer(peerId);
    }
  }
}
