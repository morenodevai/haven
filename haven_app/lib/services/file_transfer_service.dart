import 'dart:async';
import 'dart:convert';
import 'dart:ffi';

import 'package:dio/dio.dart';
import 'package:uuid/uuid.dart';

import 'package:haven_app/services/file_client_bindings.dart';
import 'package:haven_app/services/gateway_service.dart';

/// Represents a file transfer (upload or download) in progress.
class FileTransfer {
  final String transferId;
  final String filename;
  final int fileSize;
  final bool isUpload;
  final String? targetUserId;
  final String? fromUserId;

  // Native handle (null if not started yet)
  Pointer<Void>? nativeHandle;

  // Progress
  int bytesDone = 0;
  int bytesTotal = 0;
  int state = TransferState.idle;

  // File server metadata (set when offer is received/sent)
  String? fileServerUrl;
  String? fileSha256;
  List<String>? chunkHashes;

  // For uploads: true once FileOfferSend has been dispatched (after hashing)
  bool offerSent = false;
  // For uploads: true once FileUploadCompleteSend has been dispatched
  bool uploadCompleteSent = false;

  FileTransfer({
    required this.transferId,
    required this.filename,
    required this.fileSize,
    required this.isUpload,
    this.targetUserId,
    this.fromUserId,
    this.fileServerUrl,
    this.fileSha256,
    this.chunkHashes,
  });

  double get progress =>
      bytesTotal > 0 ? bytesDone / bytesTotal : 0.0;

  String get statusText {
    switch (state) {
      case TransferState.idle:
        return 'Waiting...';
      case TransferState.hashing:
        return isUpload ? 'Computing hashes...' : 'Waiting for sender...';
      case TransferState.transferring:
        return isUpload ? 'Uploading...' : 'Downloading...';
      case TransferState.complete:
        return 'Complete';
      case TransferState.error:
        return 'Error';
      case TransferState.cancelled:
        return 'Cancelled';
      default:
        return 'Unknown';
    }
  }
}

/// Orchestrates file transfers between WebSocket signaling and native
/// upload/download operations.
class FileTransferService {
  final GatewayService _gateway;
  final String Function() _getToken;
  final String Function() _getServerUrl;

  FileClientBindings? _bindings;
  final _dio = Dio();
  final Map<String, Timer> _pollTimers = {};

  FileClientBindings? _getBindings() {
    if (_bindings != null) return _bindings;
    try {
      _bindings = FileClientBindings();
      return _bindings;
    } catch (_) {
      return null;
    }
  }
  final Map<String, FileTransfer> _transfers = {};
  final Map<String, _PendingDownload> _pendingDownloads = {};
  final _uuid = const Uuid();

  Timer? _progressTimer;
  void Function()? onProgressUpdate;

  FileTransferService({
    required GatewayService gateway,
    required String Function() getToken,
    required String Function() getServerUrl,
  })  : _gateway = gateway,
        _getToken = getToken,
        _getServerUrl = getServerUrl {
    // Listen for file transfer events from gateway
    _gateway.on('FileOffer', _handleFileOffer);
    _gateway.on('FileAccept', _handleFileAccept);
    _gateway.on('FileReject', _handleFileReject);
    _gateway.on('FileReady', _handleFileReady);
  }

  Map<String, FileTransfer> get transfers => Map.unmodifiable(_transfers);

  /// Offer to send a file to a peer. Returns the transfer ID.
  String offerFile({
    required String filePath,
    required String filename,
    required int fileSize,
    required String targetUserId,
    required String masterKey,
    required String salt,
  }) {
    final transferId = _uuid.v4();

    final transfer = FileTransfer(
      transferId: transferId,
      filename: filename,
      fileSize: fileSize,
      isUpload: true,
      targetUserId: targetUserId,
    );
    _transfers[transferId] = transfer;

    // Start hashing immediately. The offer is sent AFTER pass 1 completes
    // (detected in _pollProgress) so we can include file_sha256 and chunk_hashes.
    // The receiver will then be able to start downloading in parallel with the upload.
    _startUpload(transfer, filePath, masterKey, salt);

    return transferId;
  }

  /// Accept a file offer — start downloading.
  void acceptOffer(String transferId, String savePath, String masterKey, String salt) {
    final transfer = _transfers[transferId];
    if (transfer == null) return;

    // Mark as waiting for download so it moves out of pendingOffers
    transfer.state = TransferState.hashing;
    transfer.bytesTotal = transfer.fileSize;

    // Stash save info for when FileReady arrives
    _pendingDownloads[transferId] = _PendingDownload(savePath, masterKey, salt);

    // Send accept via WebSocket
    _gateway.send({
      'type': 'FileAcceptSend',
      'data': {
        'target_user_id': transfer.fromUserId,
        'transfer_id': transferId,
      },
    });

    // Poll the file server every 5 seconds as a fallback in case FileReady
    // is never received (e.g. sender's WebSocket dropped during upload).
    _startStatusPolling(transferId);

    onProgressUpdate?.call();
  }

  /// Reject a file offer.
  void rejectOffer(String transferId) {
    final transfer = _transfers[transferId];
    if (transfer == null) return;

    _gateway.send({
      'type': 'FileRejectSend',
      'data': {
        'target_user_id': transfer.fromUserId,
        'transfer_id': transferId,
      },
    });

    _transfers.remove(transferId);
  }

  /// Cancel a transfer in progress.
  void cancelTransfer(String transferId) {
    final transfer = _transfers[transferId];
    if (transfer == null) return;

    if (transfer.nativeHandle != null) {
      _getBindings()?.cancel(transfer.nativeHandle!);
    }
    transfer.state = TransferState.cancelled;
    onProgressUpdate?.call();
  }

  /// Clean up completed/cancelled transfers.
  void removeTransfer(String transferId) {
    final transfer = _transfers.remove(transferId);
    if (transfer?.nativeHandle != null) {
      _getBindings()?.free(transfer!.nativeHandle!);
    }
  }

  void dispose() {
    _progressTimer?.cancel();
    for (final t in _pollTimers.values) {
      t.cancel();
    }
    _pollTimers.clear();
    for (final transfer in _transfers.values) {
      if (transfer.nativeHandle != null) {
        _getBindings()?.free(transfer.nativeHandle!);
      }
    }
    _transfers.clear();
  }

  void _startStatusPolling(String transferId) {
    _pollTimers[transferId]?.cancel();
    _pollTimers[transferId] = Timer.periodic(
      const Duration(seconds: 5),
      (_) => _pollTransferStatus(transferId),
    );
  }

  Future<void> _pollTransferStatus(String transferId) async {
    final transfer = _transfers[transferId];
    final pending = _pendingDownloads[transferId];
    if (transfer == null || transfer.isUpload || pending == null) {
      _pollTimers[transferId]?.cancel();
      _pollTimers.remove(transferId);
      return;
    }
    // Already started downloading
    if (transfer.nativeHandle != null) {
      _pollTimers[transferId]?.cancel();
      _pollTimers.remove(transferId);
      return;
    }

    try {
      final serverUrl = transfer.fileServerUrl ?? _getServerUrl();
      final resp = await _dio.get(
        '$serverUrl/transfers/$transferId',
        options: Options(
          headers: {'Authorization': 'Bearer ${_getToken()}'},
          receiveTimeout: const Duration(seconds: 10),
        ),
      );
      if (resp.statusCode == 200) {
        final data = resp.data as Map<String, dynamic>;
        final status = data['status'] as String?;
        if (status == 'complete') {
          _pollTimers[transferId]?.cancel();
          _pollTimers.remove(transferId);
          final p = _pendingDownloads.remove(transferId);
          if (p != null &&
              transfer.fileSha256 != null &&
              transfer.chunkHashes != null) {
            _startDownload(transfer, p.savePath, p.masterKey, p.salt);
          }
        }
      }
    } catch (_) {
      // Network error — try again next tick
    }
  }

  // ── Private methods ──────────────────────────────────────────────────

  void _startUpload(FileTransfer transfer, String filePath, String masterKey, String salt) {
    final bindings = _getBindings();
    if (bindings == null) return;
    final serverUrl = transfer.fileServerUrl ?? _getServerUrl();
    final handle = bindings.uploadFile(
      filePath: filePath,
      serverUrl: serverUrl,
      transferId: transfer.transferId,
      jwtToken: _getToken(),
      masterKey: masterKey,
      salt: salt,
    );
    transfer.nativeHandle = handle;
    _startProgressPolling();
  }

  void _startDownload(FileTransfer transfer, String savePath, String masterKey, String salt) {
    final bindings = _getBindings();
    if (bindings == null) return;
    final serverUrl = transfer.fileServerUrl?.isNotEmpty == true
        ? transfer.fileServerUrl!
        : _getServerUrl();
    final handle = bindings.downloadFile(
      savePath: savePath,
      serverUrl: serverUrl,
      transferId: transfer.transferId,
      jwtToken: _getToken(),
      masterKey: masterKey,
      salt: salt,
      fileSha256: transfer.fileSha256!,
      chunkHashesJson: jsonEncode(transfer.chunkHashes!),
    );
    transfer.nativeHandle = handle;
    _startProgressPolling();
  }

  void _startProgressPolling() {
    _progressTimer ??= Timer.periodic(
      const Duration(milliseconds: 100),
      (_) => _pollProgress(),
    );
  }

  void _pollProgress() {
    bool anyActive = false;

    for (final transfer in _transfers.values) {
      if (transfer.nativeHandle == null) continue;
      if (transfer.state == TransferState.complete ||
          transfer.state == TransferState.error ||
          transfer.state == TransferState.cancelled) {
        continue;
      }

      final bindings = _getBindings();
      if (bindings == null) continue;
      final result = bindings.getProgress(transfer.nativeHandle!);
      transfer.bytesDone = result.bytesDone;
      transfer.bytesTotal = result.bytesTotal;
      transfer.state = result.state;

      // When pass 1 (hashing) completes and pass 2 (upload) starts,
      // read the computed hashes and send the offer to the receiver.
      // This lets the receiver start downloading in parallel with our upload.
      if (transfer.isUpload &&
          !transfer.offerSent &&
          transfer.state == TransferState.transferring) {
        final hashesJson = bindings.getUploadHashesJson(transfer.nativeHandle!);
        if (hashesJson != null) {
          final data = jsonDecode(hashesJson) as Map<String, dynamic>;
          transfer.fileSha256 = data['file_sha256'] as String?;
          transfer.chunkHashes =
              (data['chunk_hashes'] as List<dynamic>).map((e) => e as String).toList();
          transfer.offerSent = true;
          _gateway.send({
            'type': 'FileOfferSend',
            'data': {
              'target_user_id': transfer.targetUserId,
              'transfer_id': transfer.transferId,
              'filename': transfer.filename,
              'size': transfer.fileSize,
              'file_sha256': transfer.fileSha256,
              'chunk_hashes': transfer.chunkHashes,
            },
          });
        }
      }

      if (transfer.state == TransferState.complete &&
          transfer.isUpload &&
          !transfer.uploadCompleteSent &&
          transfer.fileSha256 != null &&
          transfer.chunkHashes != null) {
        transfer.uploadCompleteSent = true;
        // Notify receiver that upload is complete (triggers FileReady at receiver)
        _gateway.send({
          'type': 'FileUploadCompleteSend',
          'data': {
            'target_user_id': transfer.targetUserId,
            'transfer_id': transfer.transferId,
            'file_sha256': transfer.fileSha256,
            'chunk_hashes': transfer.chunkHashes,
          },
        });
      }

      if (transfer.state != TransferState.complete &&
          transfer.state != TransferState.error &&
          transfer.state != TransferState.cancelled) {
        anyActive = true;
      }
    }

    onProgressUpdate?.call();

    if (!anyActive) {
      _progressTimer?.cancel();
      _progressTimer = null;
    }
  }

  // ── Gateway event handlers ───────────────────────────────────────────

  void _handleFileOffer(Map<String, dynamic> event) {
    final data = event['data'] as Map<String, dynamic>;
    final transferId = data['transfer_id'] as String;
    final transfer = FileTransfer(
      transferId: transferId,
      filename: data['filename'] as String,
      fileSize: data['size'] as int,
      isUpload: false,
      fromUserId: data['from_user_id'] as String,
      fileServerUrl: data['file_server_url'] as String?,
      fileSha256: data['file_sha256'] as String?,
      chunkHashes: (data['chunk_hashes'] as List<dynamic>?)
          ?.map((e) => e as String)
          .toList(),
    );
    _transfers[transferId] = transfer;
    onProgressUpdate?.call();
  }

  void _handleFileAccept(Map<String, dynamic> event) {
    // Receiver accepted — upload should already be in progress
    onProgressUpdate?.call();
  }

  void _handleFileReject(Map<String, dynamic> event) {
    final data = event['data'] as Map<String, dynamic>;
    final transferId = data['transfer_id'] as String;
    final transfer = _transfers[transferId];
    if (transfer != null) {
      cancelTransfer(transferId);
    }
  }

  void _handleFileReady(Map<String, dynamic> event) {
    final data = event['data'] as Map<String, dynamic>;
    final transferId = data['transfer_id'] as String;
    final transfer = _transfers[transferId];
    if (transfer == null || transfer.isUpload) return;

    // FileReady carries the file server URL and (on new server) the hashes.
    // Only update fileServerUrl if the new value is non-null and non-empty
    // to avoid overwriting a good URL with an empty/null one.
    final newUrl = data['file_server_url'] as String?;
    if (newUrl != null && newUrl.isNotEmpty) {
      transfer.fileServerUrl = newUrl;
    }
    // Prefer values already set from the offer; fall back to FileReady values.
    transfer.fileSha256 ??= data['file_sha256'] as String?;
    if (transfer.chunkHashes == null) {
      final hashes = data['chunk_hashes'] as List<dynamic>?;
      transfer.chunkHashes = hashes?.map((e) => e as String).toList();
    }

    // Start download if user already accepted (pending entry exists).
    // fileServerUrl may be null — _startDownload falls back to _getServerUrl().
    final pending = _pendingDownloads.remove(transferId);
    if (pending != null &&
        transfer.fileSha256 != null &&
        transfer.chunkHashes != null) {
      _startDownload(transfer, pending.savePath, pending.masterKey, pending.salt);
    }

    onProgressUpdate?.call();
  }
}

class _PendingDownload {
  final String savePath;
  final String masterKey;
  final String salt;
  _PendingDownload(this.savePath, this.masterKey, this.salt);
}
