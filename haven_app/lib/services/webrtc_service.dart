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
    final type = signal['type'] as String?;
    if (type == null) return;

    switch (type) {
      case 'TrackInfo':
        final streamId = signal['stream_id'] as String;
        final kindStr = signal['kind'] as String;
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
      'video': true,
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

  /// Start screen share and add tracks to all peers.
  Future<MediaStream?> startScreenShare() async {
    if (_screenStream != null) return _screenStream;

    _screenStream = await navigator.mediaDevices.getDisplayMedia({
      'video': true,
      'audio': false,
    });

    // Handle user cancelling the screen picker
    if (_screenStream!.getVideoTracks().isEmpty) {
      await _screenStream!.dispose();
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
        'type': 'IceCandidate',
        'candidate': candidate.candidate,
        'sdpMid': candidate.sdpMid,
        'sdpMLineIndex': candidate.sdpMLineIndex,
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
        await pc.addTrack(track, _cameraStream!);
      }
    }
    if (_screenStream != null) {
      _sendTrackInfo(peerId, _screenStream!.id, VideoTrackKind.screen);
      for (final track in _screenStream!.getTracks()) {
        await pc.addTrack(track, _screenStream!);
      }
    }

    _peers[peerId] = pc;
    return pc;
  }

  Future<void> _createAndSendOffer(String peerId) async {
    final pc = _peers[peerId];
    if (pc == null) return;

    final offer = await pc.createOffer();
    await pc.setLocalDescription(offer);

    _gateway.voiceSignalSend(peerId, {
      'type': 'Offer',
      'sdp': offer.sdp,
    });
  }

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
    await pc.setLocalDescription(answer);

    _gateway.voiceSignalSend(fromUserId, {
      'type': 'Answer',
      'sdp': answer.sdp,
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
      signal['sdpMid'] as String?,
      signal['sdpMLineIndex'] as int?,
    ));
  }

  void _sendTrackInfo(String peerId, String streamId, VideoTrackKind kind) {
    _gateway.voiceSignalSend(peerId, {
      'type': 'TrackInfo',
      'stream_id': streamId,
      'kind': kind == VideoTrackKind.screen ? 'screen' : 'camera',
    });
  }

  Future<void> _addStreamToPeers(MediaStream stream, VideoTrackKind kind) async {
    for (final entry in _peers.entries) {
      _sendTrackInfo(entry.key, stream.id, kind);
      for (final track in stream.getTracks()) {
        await entry.value.addTrack(track, stream);
      }
      // Renegotiate
      await _createAndSendOffer(entry.key);
    }
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
