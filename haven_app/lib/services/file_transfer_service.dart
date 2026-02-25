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
  // For error reporting: true once we've logged the error from the DLL
  bool errorLogged = false;

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

  /// Cached result of localhost reachability check. Null = not yet checked.
  String? _cachedUploadBaseUrl;

  FileClientBindings? _bindings;
  final _dio = Dio();
  final Map<String, Timer> _pollTimers = {};
  final Map<String, int> _pollCounts = {};

  FileClientBindings? _getBindings() {
    if (_bindings != null) return _bindings;
    try {
      _bindings = FileClientBindings();
      _log('INFO', 'FileClientBindings loaded successfully');
      return _bindings;
    } catch (e) {
      _log('ERROR', 'FileClientBindings failed to load: $e');
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

  void _log(String level, String message) {
    _gateway.send({
      'type': 'LogSend',
      'data': {'level': level, 'tag': 'FileTransfer', 'message': message},
    });
  }

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
    _log('INFO', 'offerFile: file=$filename size=$fileSize target=$targetUserId transfer=$transferId');

    final transfer = FileTransfer(
      transferId: transferId,
      filename: filename,
      fileSize: fileSize,
      isUpload: true,
      targetUserId: targetUserId,
    );
    _transfers[transferId] = transfer;

    _startUpload(transfer, filePath, masterKey, salt);

    return transferId;
  }

  /// Accept a file offer — start downloading.
  void acceptOffer(String transferId, String savePath, String masterKey, String salt) {
    final transfer = _transfers[transferId];
    if (transfer == null) {
      _log('ERROR', 'acceptOffer: transfer $transferId not found');
      return;
    }

    _log('INFO', 'acceptOffer: transfer=$transferId file=${transfer.filename} savePath=$savePath serverUrl=${transfer.fileServerUrl} sha256=${transfer.fileSha256} chunks=${transfer.chunkHashes?.length}');

    transfer.state = TransferState.hashing;
    transfer.bytesTotal = transfer.fileSize;

    _pendingDownloads[transferId] = _PendingDownload(savePath, masterKey, salt);

    _gateway.send({
      'type': 'FileAcceptSend',
      'data': {
        'target_user_id': transfer.fromUserId,
        'transfer_id': transferId,
      },
    });

    _startStatusPolling(transferId);

    onProgressUpdate?.call();
  }

  /// Reject a file offer.
  void rejectOffer(String transferId) {
    final transfer = _transfers[transferId];
    if (transfer == null) return;

    _log('INFO', 'rejectOffer: transfer=$transferId');

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

    _log('INFO', 'cancelTransfer: transfer=$transferId');

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
    _pollCounts.clear();
    for (final transfer in _transfers.values) {
      if (transfer.nativeHandle != null) {
        _getBindings()?.free(transfer.nativeHandle!);
      }
    }
    _transfers.clear();
  }

  void _startStatusPolling(String transferId) {
    _pollTimers[transferId]?.cancel();
    _pollCounts[transferId] = 0;
    _pollTimers[transferId] = Timer.periodic(
      const Duration(seconds: 5),
      (_) => _pollTransferStatus(transferId),
    );
  }

  void _stopStatusPolling(String transferId) {
    _pollTimers[transferId]?.cancel();
    _pollTimers.remove(transferId);
    _pollCounts.remove(transferId);
  }

  Future<void> _pollTransferStatus(String transferId) async {
    final transfer = _transfers[transferId];
    final pending = _pendingDownloads[transferId];
    if (transfer == null || transfer.isUpload || pending == null) {
      _stopStatusPolling(transferId);
      return;
    }
    if (transfer.nativeHandle != null) {
      _stopStatusPolling(transferId);
      return;
    }

    // Give up after 5 minutes (60 polls × 5 seconds)
    final count = (_pollCounts[transferId] ?? 0) + 1;
    _pollCounts[transferId] = count;
    if (count > 60) {
      _log('WARN', 'pollTransferStatus: transfer=$transferId timed out after $count polls, giving up');
      _stopStatusPolling(transferId);
      _pendingDownloads.remove(transferId);
      transfer.state = TransferState.error;
      onProgressUpdate?.call();
      return;
    }

    try {
      final serverUrl = transfer.fileServerUrl ?? _getServerUrl();
      _log('DEBUG', 'pollTransferStatus: transfer=$transferId url=$serverUrl poll=$count');
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
        _log('INFO', 'pollTransferStatus: transfer=$transferId status=$status');
        if (status == 'complete') {
          _stopStatusPolling(transferId);
          final p = _pendingDownloads.remove(transferId);
          if (p != null &&
              transfer.fileSha256 != null &&
              transfer.chunkHashes != null) {
            _startDownload(transfer, p.savePath, p.masterKey, p.salt);
          } else {
            _log('ERROR', 'pollTransferStatus: transfer=$transferId complete but missing hashes or pending (sha256=${transfer.fileSha256}, chunks=${transfer.chunkHashes?.length}, pending=${p != null})');
          }
        } else if (status != null && status != 'uploading') {
          // Unexpected status (e.g. failed, cancelled) — stop polling
          _log('WARN', 'pollTransferStatus: transfer=$transferId unexpected status=$status, stopping');
          _stopStatusPolling(transferId);
          _pendingDownloads.remove(transferId);
          transfer.state = TransferState.error;
          onProgressUpdate?.call();
        }
      }
    } catch (e) {
      _log('WARN', 'pollTransferStatus: transfer=$transferId error=$e');
    }
  }

  // ── Private methods ──────────────────────────────────────────────────

  void _warmLocalhostCache(String configuredUrl) {
    if (_cachedUploadBaseUrl != null) return;
    final uri = Uri.parse(configuredUrl);
    final localUrl = uri.replace(host: '127.0.0.1').toString();
    _dio.get(
      uri.replace(host: '127.0.0.1', path: '/health').toString(),
      options: Options(
        receiveTimeout: const Duration(milliseconds: 800),
        sendTimeout: const Duration(milliseconds: 800),
      ),
    ).then((_) {
      _cachedUploadBaseUrl = localUrl;
      _log('INFO', 'localhost cache: using 127.0.0.1 ($localUrl)');
    }).catchError((_) {
      _cachedUploadBaseUrl = configuredUrl;
      _log('INFO', 'localhost cache: 127.0.0.1 unreachable, using $configuredUrl');
    });
  }

  void _startUpload(FileTransfer transfer, String filePath, String masterKey, String salt) {
    final bindings = _getBindings();
    if (bindings == null) {
      _log('ERROR', '_startUpload: bindings null (DLL failed to load) transfer=${transfer.transferId}');
      return;
    }
    final configuredUrl = transfer.fileServerUrl ?? _getServerUrl();
    final serverUrl = _cachedUploadBaseUrl ?? configuredUrl;
    _warmLocalhostCache(configuredUrl);

    _log('INFO', '_startUpload: transfer=${transfer.transferId} file=$filePath server=$serverUrl');

    final handle = bindings.uploadFile(
      filePath: filePath,
      serverUrl: serverUrl,
      transferId: transfer.transferId,
      jwtToken: _getToken(),
      masterKey: masterKey,
      salt: salt,
    );
    transfer.nativeHandle = handle;
    _log('INFO', '_startUpload: native handle obtained transfer=${transfer.transferId}');
    _startProgressPolling();
  }

  void _startDownload(FileTransfer transfer, String savePath, String masterKey, String salt) {
    final bindings = _getBindings();
    if (bindings == null) {
      _log('ERROR', '_startDownload: bindings null transfer=${transfer.transferId}');
      return;
    }
    final serverUrl = transfer.fileServerUrl?.isNotEmpty == true
        ? transfer.fileServerUrl!
        : _getServerUrl();

    _log('INFO', '_startDownload: transfer=${transfer.transferId} savePath=$savePath server=$serverUrl sha256=${transfer.fileSha256} chunks=${transfer.chunkHashes?.length}');

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
    _log('INFO', '_startDownload: native handle obtained transfer=${transfer.transferId}');
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
      final prevState = transfer.state;
      transfer.bytesDone = result.bytesDone;
      transfer.bytesTotal = result.bytesTotal;
      transfer.state = result.state;

      // Log state transitions
      if (transfer.state != prevState) {
        _log('INFO', 'state transition: transfer=${transfer.transferId} ${_stateName(prevState)}->${_stateName(transfer.state)} done=${transfer.bytesDone} total=${transfer.bytesTotal}');
      }

      // When pass 1 (hashing) completes and pass 2 (upload) starts,
      // read the computed hashes and send the offer to the receiver.
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
          _log('INFO', 'hashing complete: transfer=${transfer.transferId} sha256=${transfer.fileSha256} chunks=${transfer.chunkHashes?.length} — sending FileOfferSend');
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
        } else {
          _log('WARN', 'state=transferring but getUploadHashesJson returned null: transfer=${transfer.transferId}');
        }
      }

      if (transfer.state == TransferState.complete &&
          transfer.isUpload &&
          !transfer.uploadCompleteSent &&
          transfer.fileSha256 != null &&
          transfer.chunkHashes != null) {
        transfer.uploadCompleteSent = true;
        _log('INFO', 'upload complete: transfer=${transfer.transferId} — sending FileUploadCompleteSend');
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

      // Log error with DLL error message
      if (transfer.state == TransferState.error && !transfer.errorLogged) {
        transfer.errorLogged = true;
        final errMsg = bindings.getLastError(transfer.nativeHandle!);
        _log('ERROR', 'transfer error: transfer=${transfer.transferId} isUpload=${transfer.isUpload} dllError=$errMsg done=${transfer.bytesDone} total=${transfer.bytesTotal}');
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

  String _stateName(int s) {
    switch (s) {
      case TransferState.idle: return 'idle';
      case TransferState.hashing: return 'hashing';
      case TransferState.transferring: return 'transferring';
      case TransferState.complete: return 'complete';
      case TransferState.error: return 'error';
      case TransferState.cancelled: return 'cancelled';
      default: return 'unknown($s)';
    }
  }

  // ── Gateway event handlers ───────────────────────────────────────────

  void _handleFileOffer(Map<String, dynamic> event) {
    final data = event['data'] as Map<String, dynamic>;
    final transferId = data['transfer_id'] as String;
    final filename = data['filename'] as String;
    final size = data['size'] as int;
    final fromUserId = data['from_user_id'] as String;
    final fileServerUrl = data['file_server_url'] as String?;
    final fileSha256 = data['file_sha256'] as String?;
    final chunkHashes = (data['chunk_hashes'] as List<dynamic>?)
        ?.map((e) => e as String)
        .toList();

    _log('INFO', '_handleFileOffer: transfer=$transferId file=$filename size=$size from=$fromUserId serverUrl=$fileServerUrl sha256=$fileSha256 chunks=${chunkHashes?.length}');

    final transfer = FileTransfer(
      transferId: transferId,
      filename: filename,
      fileSize: size,
      isUpload: false,
      fromUserId: fromUserId,
      fileServerUrl: fileServerUrl,
      fileSha256: fileSha256,
      chunkHashes: chunkHashes,
    );
    _transfers[transferId] = transfer;
    onProgressUpdate?.call();
  }

  void _handleFileAccept(Map<String, dynamic> event) {
    final data = event['data'] as Map<String, dynamic>? ?? {};
    final transferId = data['transfer_id'] as String? ?? '?';
    _log('INFO', '_handleFileAccept: transfer=$transferId — receiver accepted');
    onProgressUpdate?.call();
  }

  void _handleFileReject(Map<String, dynamic> event) {
    final data = event['data'] as Map<String, dynamic>;
    final transferId = data['transfer_id'] as String;
    _log('INFO', '_handleFileReject: transfer=$transferId — receiver rejected');
    final transfer = _transfers[transferId];
    if (transfer != null) {
      cancelTransfer(transferId);
    }
  }

  void _handleFileReady(Map<String, dynamic> event) {
    final data = event['data'] as Map<String, dynamic>;
    final transferId = data['transfer_id'] as String;
    final transfer = _transfers[transferId];

    _log('INFO', '_handleFileReady: transfer=$transferId fileServerUrl=${data['file_server_url']} sha256=${data['file_sha256']} chunks=${(data['chunk_hashes'] as List<dynamic>?)?.length}');

    if (transfer == null || transfer.isUpload) {
      _log('WARN', '_handleFileReady: transfer=$transferId not found or is upload — ignoring');
      return;
    }

    final newUrl = data['file_server_url'] as String?;
    if (newUrl != null && newUrl.isNotEmpty) {
      transfer.fileServerUrl = newUrl;
    }
    transfer.fileSha256 ??= data['file_sha256'] as String?;
    if (transfer.chunkHashes == null) {
      final hashes = data['chunk_hashes'] as List<dynamic>?;
      transfer.chunkHashes = hashes?.map((e) => e as String).toList();
    }

    final pending = _pendingDownloads.remove(transferId);
    _log('INFO', '_handleFileReady: pending=${pending != null} sha256=${transfer.fileSha256} chunks=${transfer.chunkHashes?.length}');
    if (pending != null &&
        transfer.fileSha256 != null &&
        transfer.chunkHashes != null) {
      _startDownload(transfer, pending.savePath, pending.masterKey, pending.salt);
    } else {
      _log('WARN', '_handleFileReady: not starting download — pending=${pending != null} sha256=${transfer.fileSha256} chunks=${transfer.chunkHashes?.length}');
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
