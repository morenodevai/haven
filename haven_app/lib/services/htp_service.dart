// High-level Haven Transfer Protocol service.
//
// Orchestrates file transfers by:
//   1. Sending OFFER/ACCEPT/CANCEL via WebSocket (control channel)
//   2. Delegating UDP data transfer to the native haven-transfer library via FFI
///      (native code handles UDP relay authentication internally)
///   3. Bridging NACK/RTT/ACK messages between WebSocket and native layer
///   4. Providing progress streams for the UI

import 'dart:async';
import 'dart:convert';
import 'dart:ffi';
import 'dart:io';

import 'package:ffi/ffi.dart';

import 'htp_bindings.dart';
import 'gateway_service.dart';
import 'auth_service.dart';
import '../config/constants.dart';

/// Transfer direction.
enum TransferDirection { sending, receiving }

/// Transfer state.
enum TransferState { pending, active, complete, failed, cancelled }

/// A tracked file transfer.
class HtpTransfer {
  final int sessionId;
  final String filename;
  final int fileSize;
  final TransferDirection direction;
  final String? localPath;

  TransferState state = TransferState.pending;
  double progress = 0.0;
  int rateBps = 0;
  int retransmits = 0;
  String? error;
  DateTime startTime = DateTime.now();

  HtpTransfer({
    required this.sessionId,
    required this.filename,
    required this.fileSize,
    required this.direction,
    this.localPath,
  });

  Duration get elapsed => DateTime.now().difference(startTime);

  String get rateFormatted {
    if (rateBps > 1000000000) {
      return '${(rateBps / 1000000000).toStringAsFixed(1)} GB/s';
    } else if (rateBps > 1000000) {
      return '${(rateBps / 1000000).toStringAsFixed(1)} MB/s';
    } else if (rateBps > 1000) {
      return '${(rateBps / 1000).toStringAsFixed(1)} KB/s';
    }
    return '$rateBps B/s';
  }

  String get sizeFormatted {
    if (fileSize > 1000000000) {
      return '${(fileSize / 1000000000).toStringAsFixed(2)} GB';
    } else if (fileSize > 1000000) {
      return '${(fileSize / 1000000).toStringAsFixed(1)} MB';
    } else if (fileSize > 1000) {
      return '${(fileSize / 1000).toStringAsFixed(1)} KB';
    }
    return '$fileSize B';
  }
}

/// The HTP service. Manages active transfers and bridges control messages.
class HtpService {
  final GatewayService _gateway;
  final AuthService _auth;
  final HtpBindings _htp;
  final Map<int, HtpTransfer> _transfers = {};
  Timer? _pollTimer;

  /// Stream of transfer state changes for UI updates.
  final _transferController = StreamController<HtpTransfer>.broadcast();
  Stream<HtpTransfer> get transferStream => _transferController.stream;

  /// Pending incoming offers waiting for user acceptance.
  final _offerController =
      StreamController<Map<String, dynamic>>.broadcast();
  Stream<Map<String, dynamic>> get offerStream => _offerController.stream;

  HtpService(this._gateway, this._auth) : _htp = HtpBindings() {
    _pollTimer = Timer.periodic(const Duration(milliseconds: 200), _pollStats);
  }

  /// Get the relay address string (host:port) for the UDP relay.
  String get _relayAddr {
    final serverHost = Uri.parse(_auth.serverUrl).host;
    final mainPort = Uri.parse(_auth.serverUrl).port;
    final relayPort = mainPort + 1; // 3211
    return '$serverHost:$relayPort';
  }

  /// Send a file to a specific user via the server relay.
  Future<HtpTransfer?> sendFile({
    required String filePath,
    required String recipientId,
  }) async {
    final file = File(filePath);
    if (!await file.exists()) return null;

    final fileSize = await file.length();
    final chunkSize = _htp.chunkSize();
    final totalChunks = (fileSize + chunkSize - 1) ~/ chunkSize;
    final filename = file.uri.pathSegments.last;

    // Generate session salt
    final salt = _htp.randomSalt();
    final saltBase64 = base64.encode(salt);

    // Create transfer record
    final sessionId = DateTime.now().millisecondsSinceEpoch ~/ 1000;
    final transfer = HtpTransfer(
      sessionId: sessionId,
      filename: filename,
      fileSize: fileSize,
      direction: TransferDirection.sending,
      localPath: filePath,
    );
    _transfers[sessionId] = transfer;
    _transferController.add(transfer);

    // Send HTP OFFER via WebSocket control channel
    _gateway.send({
      'type': 'HtpOfferSend',
      'data': {
        'target_user_id': recipientId,
        'session_id': sessionId,
        'filename': filename,
        'size': fileSize,
        'chunk_count': totalChunks,
        'salt': saltBase64,
      },
    });

    return transfer;
  }

  /// Accept an incoming transfer offer.
  Future<void> acceptOffer(Map<String, dynamic> offer, String savePath) async {
    final sessionId = offer['session_id'] as int;
    final fileSize = offer['size'] as int;
    final totalChunks = offer['chunk_count'] as int;
    final filename = offer['filename'] as String;
    final saltBase64 = offer['salt'] as String;

    final transfer = HtpTransfer(
      sessionId: sessionId,
      filename: filename,
      fileSize: fileSize,
      direction: TransferDirection.receiving,
      localPath: savePath,
    );
    _transfers[sessionId] = transfer;

    // Send HTP ACCEPT via WebSocket control channel
    final senderId = offer['from_user_id'] as String;
    _gateway.send({
      'type': 'HtpAcceptSend',
      'data': {
        'target_user_id': senderId,
        'session_id': sessionId,
      },
    });

    // Start native receiver
    _startReceiver(
      sessionId: sessionId,
      outputPath: savePath,
      fileSize: fileSize,
      totalChunks: totalChunks,
      saltBase64: saltBase64,
    );

    transfer.state = TransferState.active;
    transfer.startTime = DateTime.now();
    _transferController.add(transfer);
  }

  /// Reject an incoming offer.
  void rejectOffer(Map<String, dynamic> offer) {
    final senderId = offer['from_user_id'] as String;
    _gateway.send({
      'type': 'HtpCancelSend',
      'data': {
        'target_user_id': senderId,
        'session_id': offer['session_id'],
        'reason': 'rejected',
      },
    });
  }

  /// Cancel an active transfer.
  void cancelTransfer(int sessionId) {
    _htp.cancel(sessionId);
    final transfer = _transfers[sessionId];
    if (transfer != null) {
      transfer.state = TransferState.cancelled;
      _transferController.add(transfer);
    }
    _gateway.send({
      'type': 'HtpCancelSend',
      'data': {
        'session_id': sessionId,
        'reason': 'cancelled',
      },
    });
  }

  /// Handle control messages from the WebSocket gateway.
  /// Supports both legacy transfer_* events and new HTP-specific events.
  void handleControlMessage(Map<String, dynamic> msg) {
    final type = msg['type'] as String?;
    if (type == null) return;

    // The server wraps events as {"type":"EventName","data":{...}}
    // Extract the inner data map when present.
    final data = msg['data'] as Map<String, dynamic>? ?? msg;

    switch (type) {
      // Legacy WebSocket relay events
      case 'transfer_offer':
        _offerController.add(msg);
        break;
      case 'transfer_accept':
        _onAccept(msg);
        break;
      case 'transfer_nack':
        _onNack(data);
        break;
      case 'transfer_ack':
        _onAck(data);
        break;
      case 'transfer_rtt':
        _onRtt(data);
        break;
      case 'transfer_done':
        _onDone(data);
        break;
      case 'transfer_cancel':
        _onCancel(data);
        break;

      // HTP control events (from server gateway)
      case 'HtpOffer':
        _offerController.add(data);
        break;
      case 'HtpAccept':
        _onAccept(data);
        break;
      case 'HtpNack':
        _onHtpNack(data);
        break;
      case 'HtpRtt':
        _onHtpRtt(data);
        break;
      case 'HtpAck':
        _onHtpAck(data);
        break;
      case 'HtpDone':
        _onHtpDone(data);
        break;
      case 'HtpCancel':
        _onHtpCancel(data);
        break;
    }
  }

  // ── Internal ──

  void _onAccept(Map<String, dynamic> msg) {
    final sessionId = msg['session_id'] as int;
    final transfer = _transfers[sessionId];
    if (transfer == null || transfer.direction != TransferDirection.sending) {
      return;
    }

    final serverHost = Uri.parse(_auth.serverUrl).host;
    final mainPort = Uri.parse(_auth.serverUrl).port;
    final relayPort = mainPort + 1;
    final destAddr = '$serverHost:$relayPort';

    _startSender(
      sessionId: sessionId,
      filePath: transfer.localPath!,
      destAddr: destAddr,
    );

    transfer.state = TransferState.active;
    transfer.startTime = DateTime.now();
    _transferController.add(transfer);
  }

  void _onNack(Map<String, dynamic> msg) {
    final sessionId = msg['session_id'] as int;
    final missing = (msg['missing'] as List<dynamic>)
        .map((e) => (e as num).toInt())
        .toList();

    if (missing.isEmpty) return;

    final pMissing = calloc<Uint64>(missing.length);
    for (var i = 0; i < missing.length; i++) {
      pMissing[i] = missing[i];
    }
    _htp.senderNack(sessionId, pMissing, missing.length);
    calloc.free(pMissing);
  }

  void _onAck(Map<String, dynamic> msg) {
    final sessionId = msg['session_id'] as int;
    final upTo = (msg['up_to'] as num).toInt();
    _htp.senderAck(sessionId, upTo);
  }

  void _onRtt(Map<String, dynamic> msg) {
    final sessionId = msg['session_id'] as int;
    final rttUs = (msg['rtt_us'] as num).toInt();
    _htp.senderRtt(sessionId, rttUs);
  }

  void _onDone(Map<String, dynamic> msg) {
    final sessionId = msg['session_id'] as int;
    final transfer = _transfers[sessionId];
    if (transfer != null) {
      transfer.state = TransferState.complete;
      transfer.progress = 1.0;
      _transferController.add(transfer);
    }
  }

  void _onCancel(Map<String, dynamic> msg) {
    final sessionId = msg['session_id'] as int;
    _htp.cancel(sessionId);
    final transfer = _transfers[sessionId];
    if (transfer != null) {
      transfer.state = TransferState.cancelled;
      _transferController.add(transfer);
    }
  }

  // ── HTP-specific event handlers ──

  void _onHtpNack(Map<String, dynamic> data) {
    final sessionId = data['session_id'] as int;
    final missing = (data['missing'] as List<dynamic>)
        .map((e) => (e as num).toInt())
        .toList();
    if (missing.isEmpty) return;

    final pMissing = calloc<Uint64>(missing.length);
    for (var i = 0; i < missing.length; i++) {
      pMissing[i] = missing[i];
    }
    _htp.senderNack(sessionId, pMissing, missing.length);
    calloc.free(pMissing);
  }

  void _onHtpRtt(Map<String, dynamic> data) {
    final sessionId = data['session_id'] as int;
    final rttUs = (data['rtt_us'] as num).toInt();
    _htp.senderRtt(sessionId, rttUs);
  }

  void _onHtpAck(Map<String, dynamic> data) {
    final sessionId = data['session_id'] as int;
    final upTo = (data['up_to'] as num).toInt();
    _htp.senderAck(sessionId, upTo);
  }

  void _onHtpDone(Map<String, dynamic> data) {
    final sessionId = data['session_id'] as int;
    final transfer = _transfers[sessionId];
    if (transfer != null) {
      transfer.state = TransferState.complete;
      transfer.progress = 1.0;
      _transferController.add(transfer);
    }
  }

  void _onHtpCancel(Map<String, dynamic> data) {
    final sessionId = data['session_id'] as int;
    _htp.cancel(sessionId);
    final transfer = _transfers[sessionId];
    if (transfer != null) {
      transfer.state = TransferState.cancelled;
      _transferController.add(transfer);
    }
  }

  // ── HTP control message senders (Dart → WebSocket → Server → Peer) ──

  /// Send NACK list to the sender via WebSocket control channel.
  void sendHtpNack(int sessionId, String targetUserId, List<int> missing) {
    _gateway.send({
      'type': 'HtpNackSend',
      'data': {
        'target_user_id': targetUserId,
        'session_id': sessionId,
        'missing': missing,
      },
    });
  }

  /// Send RTT measurement to the sender via WebSocket control channel.
  void sendHtpRtt(int sessionId, String targetUserId, int rttUs) {
    _gateway.send({
      'type': 'HtpRttSend',
      'data': {
        'target_user_id': targetUserId,
        'session_id': sessionId,
        'rtt_us': rttUs,
      },
    });
  }

  /// Send ACK (progress) to the sender via WebSocket control channel.
  void sendHtpAck(int sessionId, String targetUserId, int upTo) {
    _gateway.send({
      'type': 'HtpAckSend',
      'data': {
        'target_user_id': targetUserId,
        'session_id': sessionId,
        'up_to': upTo,
      },
    });
  }

  /// Signal transfer complete via WebSocket control channel.
  void sendHtpDone(int sessionId, String targetUserId, int totalBytes) {
    _gateway.send({
      'type': 'HtpDoneSend',
      'data': {
        'target_user_id': targetUserId,
        'session_id': sessionId,
        'total_bytes': totalBytes,
      },
    });
  }

  void _startSender({
    required int sessionId,
    required String filePath,
    required String destAddr,
  }) {
    final pPath = filePath.toNativeUtf8();
    final pAddr = destAddr.toNativeUtf8();
    final pKey = calloc<Uint8>(32);
    final pSalt = calloc<Uint8>(32);

    final keyBytes = base64.decode(HavenConstants.defaultChannelKey);
    for (var i = 0; i < 32 && i < keyBytes.length; i++) {
      pKey[i] = keyBytes[i];
    }

    final saltBytes = _htp.randomSalt();
    for (var i = 0; i < 32; i++) {
      pSalt[i] = saltBytes[i];
    }

    // Pass JWT token so the native sender can authenticate with the UDP relay
    final token = _auth.token ?? '';
    final pToken = token.toNativeUtf8();

    _htp.sendFile(pPath, pAddr, pKey, pSalt, pToken);

    calloc.free(pPath);
    calloc.free(pAddr);
    calloc.free(pKey);
    calloc.free(pSalt);
    calloc.free(pToken);
  }

  void _startReceiver({
    required int sessionId,
    required String outputPath,
    required int fileSize,
    required int totalChunks,
    required String saltBase64,
  }) {
    final pPath = outputPath.toNativeUtf8();
    final pAddr = '0.0.0.0:0'.toNativeUtf8();
    final pKey = calloc<Uint8>(32);
    final pSalt = calloc<Uint8>(32);

    final keyBytes = base64.decode(HavenConstants.defaultChannelKey);
    for (var i = 0; i < 32 && i < keyBytes.length; i++) {
      pKey[i] = keyBytes[i];
    }

    final saltBytes = base64.decode(saltBase64);
    for (var i = 0; i < 32 && i < saltBytes.length; i++) {
      pSalt[i] = saltBytes[i];
    }

    // Pass JWT token and relay address so the native receiver can authenticate
    final token = _auth.token ?? '';
    final pToken = token.toNativeUtf8();
    final pRelay = _relayAddr.toNativeUtf8();

    _htp.recvFile(
      sessionId,
      pPath,
      fileSize,
      totalChunks,
      pAddr,
      pKey,
      pSalt,
      pToken,
      pRelay,
      nullptr,
      nullptr,
    );

    calloc.free(pPath);
    calloc.free(pAddr);
    calloc.free(pKey);
    calloc.free(pSalt);
    calloc.free(pToken);
    calloc.free(pRelay);
  }

  void _pollStats(Timer _) {
    for (final entry in _transfers.entries) {
      final transfer = entry.value;
      if (transfer.state != TransferState.active) continue;

      final stats = _htp.getStats(entry.key);
      if (stats == null) {
        if (transfer.state == TransferState.active) {
          transfer.state = transfer.progress >= 0.99
              ? TransferState.complete
              : TransferState.failed;
          _transferController.add(transfer);
        }
        continue;
      }

      transfer.progress = stats.progress;
      transfer.rateBps = stats.rateBps;
      transfer.retransmits = stats.retransmits;
      _transferController.add(transfer);
    }
  }

  List<HtpTransfer> get activeTransfers =>
      _transfers.values.where((t) => t.state == TransferState.active).toList();

  Map<int, HtpTransfer> get allTransfers => Map.unmodifiable(_transfers);

  void dispose() {
    _pollTimer?.cancel();
    _transferController.close();
    _offerController.close();
    for (final entry in _transfers.entries) {
      if (entry.value.state == TransferState.active) {
        _htp.cancel(entry.key);
      }
    }
  }
}
