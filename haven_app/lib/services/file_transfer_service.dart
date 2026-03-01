import 'dart:async';
import 'dart:convert';
import 'dart:ffi';
import 'dart:io';

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

  /// If this file belongs to a folder transfer, the parent folder ID.
  final String? folderId;

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
    this.folderId,
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

/// Folder transfer state machine.
class FolderTransferState {
  static const int pending = 0;
  static const int active = 1;
  static const int complete = 2;
  static const int error = 3;
  static const int rejected = 4;
}

/// A single entry in the folder manifest.
class FolderFileEntry {
  final String relativePath;
  final int size;
  FolderFileEntry({required this.relativePath, required this.size});
}

/// Represents a folder transfer — groups multiple child FileTransfers.
class FolderTransfer {
  final String folderId;
  final String folderName;
  final int totalSize;
  final int fileCount;
  final bool isUpload;
  final String? fromUserId;
  final String? targetUserId;
  final List<FolderFileEntry> manifest;

  int state = FolderTransferState.pending;

  /// Maps child transfer IDs to their relative paths in the folder.
  final Map<String, String> transferIdToPath = {};

  // Stored after accept (receiver side) for auto-accepting child files.
  String? saveDir;
  String? masterKey;
  String? salt;

  FolderTransfer({
    required this.folderId,
    required this.folderName,
    required this.totalSize,
    required this.fileCount,
    required this.isUpload,
    required this.manifest,
    this.fromUserId,
    this.targetUserId,
  });

  /// Compute progress from child transfers.
  double progress(Map<String, FileTransfer> allTransfers) {
    if (totalSize == 0) return 0.0;
    return bytesDone(allTransfers) / totalSize;
  }

  int bytesDone(Map<String, FileTransfer> allTransfers) {
    int done = 0;
    for (final tid in transferIdToPath.keys) {
      final t = allTransfers[tid];
      if (t != null) done += t.bytesDone;
    }
    return done;
  }

  int filesComplete(Map<String, FileTransfer> allTransfers) {
    int count = 0;
    for (final tid in transferIdToPath.keys) {
      final t = allTransfers[tid];
      if (t != null && t.state == TransferState.complete) count++;
    }
    return count;
  }

  /// The currently active file (first one that is hashing or transferring).
  FileTransfer? activeFile(Map<String, FileTransfer> allTransfers) {
    for (final tid in transferIdToPath.keys) {
      final t = allTransfers[tid];
      if (t != null &&
          (t.state == TransferState.hashing ||
           t.state == TransferState.transferring)) {
        return t;
      }
    }
    return null;
  }
}

/// Orchestrates file transfers between WebSocket signaling and native
/// upload/download operations.
class FileTransferService {
  final GatewayService _gateway;
  final String Function() _getToken;
  final String Function() _getServerUrl;

  /// Cached result of localhost reachability check. Null = not yet checked.


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
  final Map<String, FolderTransfer> _folders = {};
  final Map<String, _PendingDownload> _pendingDownloads = {};
  final _uuid = const Uuid();

  // Send queue: files waiting to be uploaded (sequential to avoid UDP blast collision)
  final List<_QueuedSend> _sendQueue = [];
  String? _activeUploadTransferId;

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
    _gateway.on('FastProgress', _handleFastProgress);
    _gateway.on('FolderOffer', _handleFolderOffer);
    _gateway.on('FolderAccept', _handleFolderAccept);
    _gateway.on('FolderReject', _handleFolderReject);
  }

  Map<String, FileTransfer> get transfers => Map.unmodifiable(_transfers);
  Map<String, FolderTransfer> get folders => Map.unmodifiable(_folders);

  void _log(String level, String message) {
    _gateway.send({
      'type': 'LogSend',
      'data': {'level': level, 'tag': 'FileTransfer', 'message': message},
    });
  }

  /// Offer to send a file to a peer. Returns the transfer ID.
  /// Files are queued and sent sequentially to avoid UDP blast collisions.
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

    if (_activeUploadTransferId == null) {
      // No active upload — start immediately
      _activeUploadTransferId = transferId;
      _startUpload(transfer, filePath, masterKey, salt);
    } else {
      // Queue it
      _log('INFO', 'offerFile: queued (active=$_activeUploadTransferId) transfer=$transferId');
      _sendQueue.add(_QueuedSend(
        transferId: transferId,
        filePath: filePath,
        masterKey: masterKey,
        salt: salt,
      ));
    }

    return transferId;
  }

  /// Send a folder to a peer. Builds manifest, sends FolderOfferSend,
  /// then queues individual file uploads tagged with the folder ID.
  /// Returns the folder ID.
  String? offerFolder({
    required String folderPath,
    required String targetUserId,
    required String masterKey,
    required String salt,
  }) {
    final dir = Directory(folderPath);
    if (!dir.existsSync()) {
      _log('ERROR', 'offerFolder: directory not found: $folderPath');
      return null;
    }

    final files = dir
        .listSync(recursive: true)
        .whereType<File>()
        .toList()
      ..sort((a, b) => a.path.compareTo(b.path));

    if (files.isEmpty) {
      _log('WARN', 'offerFolder: no files found in $folderPath');
      return null;
    }

    final folderId = _uuid.v4();
    final basePath = folderPath.replaceAll('\\', '/');
    final folderName = basePath.split('/').last;

    // Build manifest
    final manifest = <FolderFileEntry>[];
    int totalSize = 0;
    for (final file in files) {
      final fullPath = file.path.replaceAll('\\', '/');
      final relativePath = fullPath.startsWith(basePath)
          ? fullPath.substring(basePath.length + 1)
          : fullPath.split('/').last;
      final size = file.lengthSync();
      manifest.add(FolderFileEntry(relativePath: relativePath, size: size));
      totalSize += size;
    }

    _log('INFO', 'offerFolder: $folderName ${files.length} files ${totalSize} bytes → $targetUserId folder=$folderId');

    // Create FolderTransfer (upload side, immediately active)
    final folder = FolderTransfer(
      folderId: folderId,
      folderName: folderName,
      totalSize: totalSize,
      fileCount: files.length,
      isUpload: true,
      manifest: manifest,
      targetUserId: targetUserId,
    );
    folder.state = FolderTransferState.active;
    _folders[folderId] = folder;

    // Send FolderOfferSend via gateway
    _gateway.send({
      'type': 'FolderOfferSend',
      'data': {
        'target_user_id': targetUserId,
        'folder_id': folderId,
        'folder_name': folderName,
        'total_size': totalSize,
        'file_count': files.length,
        'manifest': manifest
            .map((e) => {'relative_path': e.relativePath, 'size': e.size})
            .toList(),
      },
    });

    // Queue individual file uploads tagged with folderId
    for (int i = 0; i < files.length; i++) {
      final file = files[i];
      final entry = manifest[i];
      final transferId = _uuid.v4();

      final transfer = FileTransfer(
        transferId: transferId,
        filename: entry.relativePath,
        fileSize: entry.size,
        isUpload: true,
        targetUserId: targetUserId,
        folderId: folderId,
      );
      _transfers[transferId] = transfer;
      folder.transferIdToPath[transferId] = entry.relativePath;

      if (_activeUploadTransferId == null && i == 0) {
        _activeUploadTransferId = transferId;
        _startUpload(transfer, file.path, masterKey, salt);
      } else {
        _sendQueue.add(_QueuedSend(
          transferId: transferId,
          filePath: file.path,
          masterKey: masterKey,
          salt: salt,
        ));
      }
    }

    onProgressUpdate?.call();
    return folderId;
  }

  /// Start the next queued upload if the current one is done.
  void _processQueue() {
    if (_sendQueue.isEmpty) {
      _activeUploadTransferId = null;
      return;
    }

    final next = _sendQueue.removeAt(0);
    final transfer = _transfers[next.transferId];
    if (transfer == null) {
      // Transfer was cancelled/removed — skip
      _processQueue();
      return;
    }

    _activeUploadTransferId = next.transferId;
    _log('INFO', '_processQueue: starting next upload transfer=${next.transferId} remaining=${_sendQueue.length}');
    _startUpload(transfer, next.filePath, next.masterKey, next.salt);
  }

  /// Accept a file offer — start downloading.
  void acceptOffer(String transferId, String savePath, String masterKey, String salt) {
    final transfer = _transfers[transferId];
    if (transfer == null) {
      _log('ERROR', 'acceptOffer: transfer $transferId not found');
      return;
    }

    _log('INFO', 'acceptOffer: transfer=$transferId file=${transfer.filename} savePath=$savePath serverUrl=${transfer.fileServerUrl} sha256=${transfer.fileSha256} chunks=${transfer.chunkHashes?.length}');

    // Ensure parent directory exists (for folder transfers with nested paths)
    final parentDir = Directory(savePath.substring(0, savePath.lastIndexOf(RegExp(r'[/\\]'))));
    if (!parentDir.existsSync()) {
      parentDir.createSync(recursive: true);
    }

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

  /// Accept a folder offer — store save dir and credentials, auto-accept
  /// any child FileOffers already received, send FolderAcceptSend.
  void acceptFolder(String folderId, String saveDir, String masterKey, String salt) {
    final folder = _folders[folderId];
    if (folder == null) {
      _log('ERROR', 'acceptFolder: folder $folderId not found');
      return;
    }

    _log('INFO', 'acceptFolder: folder=$folderId name=${folder.folderName} saveDir=$saveDir');

    folder.state = FolderTransferState.active;
    folder.saveDir = saveDir;
    folder.masterKey = masterKey;
    folder.salt = salt;

    // Send FolderAcceptSend
    _gateway.send({
      'type': 'FolderAcceptSend',
      'data': {
        'target_user_id': folder.fromUserId,
        'folder_id': folderId,
      },
    });

    // Auto-accept any child file offers already received
    for (final entry in folder.transferIdToPath.entries) {
      final transfer = _transfers[entry.key];
      if (transfer != null && !transfer.isUpload && transfer.state == TransferState.idle) {
        final savePath = '$saveDir/${entry.value}'.replaceAll('/', '\\');
        acceptOffer(entry.key, savePath, masterKey, salt);
      }
    }

    onProgressUpdate?.call();
  }

  /// Reject a folder offer — cancel all child transfers.
  void rejectFolder(String folderId) {
    final folder = _folders[folderId];
    if (folder == null) return;

    _log('INFO', 'rejectFolder: folder=$folderId');

    folder.state = FolderTransferState.rejected;

    // Send FolderRejectSend
    _gateway.send({
      'type': 'FolderRejectSend',
      'data': {
        'target_user_id': folder.fromUserId,
        'folder_id': folderId,
      },
    });

    // Cancel/remove all child transfers
    for (final tid in folder.transferIdToPath.keys) {
      final transfer = _transfers[tid];
      if (transfer != null) {
        if (transfer.nativeHandle != null) {
          _getBindings()?.cancel(transfer.nativeHandle!);
        }
        _transfers.remove(tid);
      }
    }

    onProgressUpdate?.call();
  }

  /// Cancel a folder transfer and remove it + all children immediately.
  void cancelAndRemoveFolder(String folderId) {
    final folder = _folders[folderId];
    if (folder == null) return;

    _log('INFO', 'cancelAndRemoveFolder: folder=$folderId name=${folder.folderName}');

    // Cancel + free all child transfers
    for (final tid in folder.transferIdToPath.keys) {
      final transfer = _transfers[tid];
      if (transfer != null) {
        if (transfer.nativeHandle != null) {
          _getBindings()?.cancel(transfer.nativeHandle!);
          _getBindings()?.free(transfer.nativeHandle!);
        }
        _stopStatusPolling(tid);
        _pendingDownloads.remove(tid);
      }
      _transfers.remove(tid);
      // Remove from send queue too
      _sendQueue.removeWhere((q) => q.transferId == tid);
    }

    // If active upload was a child of this folder, advance queue
    if (folder.transferIdToPath.containsKey(_activeUploadTransferId)) {
      _activeUploadTransferId = null;
      _processQueue();
    }

    _folders.remove(folderId);
    onProgressUpdate?.call();
  }

  /// Remove a completed/rejected folder and its child transfers.
  void removeFolder(String folderId) {
    final folder = _folders.remove(folderId);
    if (folder == null) return;
    for (final tid in folder.transferIdToPath.keys) {
      removeTransfer(tid);
    }
  }

  /// Cancel a transfer in progress and remove it immediately.
  void cancelAndRemoveTransfer(String transferId) {
    final transfer = _transfers[transferId];
    if (transfer == null) return;

    _log('INFO', 'cancelAndRemoveTransfer: transfer=$transferId');

    if (transfer.nativeHandle != null) {
      _getBindings()?.cancel(transfer.nativeHandle!);
      _getBindings()?.free(transfer.nativeHandle!);
    }
    _stopStatusPolling(transferId);
    _pendingDownloads.remove(transferId);
    _sendQueue.removeWhere((q) => q.transferId == transferId);
    _transfers.remove(transferId);

    // Advance queue if this was the active upload
    if (_activeUploadTransferId == transferId) {
      _activeUploadTransferId = null;
      _processQueue();
    }

    onProgressUpdate?.call();
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
    _folders.clear();
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
      final serverUrl = _getServerUrl();
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

  Future<String> _resolveServerUrl(String configuredUrl) async {
    final uri = Uri.parse(configuredUrl);
    final healthOpts = Options(
      receiveTimeout: const Duration(milliseconds: 800),
      sendTimeout: const Duration(milliseconds: 800),
    );

    // Try localhost first (same machine as server)
    try {
      await _dio.get(uri.replace(host: '127.0.0.1', path: '/health').toString(), options: healthOpts);
      final resolved = uri.replace(host: '127.0.0.1').toString();
      _log('INFO', 'server resolve: using 127.0.0.1 ($resolved)');
      return resolved;
    } catch (_) {}

    // Try the chat server's host (LAN IP the app already connected to)
    final chatHost = Uri.parse(_getServerUrl()).host;
    if (chatHost.isNotEmpty && chatHost != uri.host && chatHost != '127.0.0.1') {
      try {
        await _dio.get(uri.replace(host: chatHost, path: '/health').toString(), options: healthOpts);
        final resolved = uri.replace(host: chatHost).toString();
        _log('INFO', 'server resolve: using chat host $chatHost ($resolved)');
        return resolved;
      } catch (_) {}
    }

    _log('INFO', 'server resolve: using configured $configuredUrl');
    return configuredUrl;
  }

  void _startUpload(FileTransfer transfer, String filePath, String masterKey, String salt) async {
    final bindings = _getBindings();
    if (bindings == null) {
      _log('ERROR', '_startUpload: bindings null (DLL failed to load) transfer=${transfer.transferId}');
      return;
    }
    final serverUrl = _getServerUrl();

    _log('INFO', '_startUpload: transfer=${transfer.transferId} file=$filePath server=$serverUrl');

    // Use HTTP upload (works through file gateway proxy for NAT traversal)
    final handle = bindings.uploadFile(
      filePath: filePath,
      serverUrl: serverUrl,
      transferId: transfer.transferId,
      jwtToken: _getToken(),
      masterKey: masterKey,
      salt: salt,
    );
    transfer.nativeHandle = handle;
    _log('INFO', '_startUpload: HTTP upload handle obtained transfer=${transfer.transferId}');
    _startProgressPolling();
  }

  void _startDownload(FileTransfer transfer, String savePath, String masterKey, String salt) async {
    final bindings = _getBindings();
    if (bindings == null) {
      _log('ERROR', '_startDownload: bindings null transfer=${transfer.transferId}');
      return;
    }
    final serverUrl = _getServerUrl();

    _log('INFO', '_startDownload: transfer=${transfer.transferId} savePath=$savePath server=$serverUrl sha256=${transfer.fileSha256} chunks=${transfer.chunkHashes?.length}');

    // Use HTTP download (works through file gateway proxy for NAT traversal)
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
    _log('INFO', '_startDownload: HTTP download handle obtained transfer=${transfer.transferId}');
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
      // Also handles tiny files that jump straight to complete (skip transferring).
      if (transfer.isUpload &&
          !transfer.offerSent &&
          (transfer.state == TransferState.transferring ||
           transfer.state == TransferState.complete)) {
        final hashesJson = bindings.getUploadHashesJson(transfer.nativeHandle!);
        if (hashesJson != null) {
          final data = jsonDecode(hashesJson) as Map<String, dynamic>;
          transfer.fileSha256 = data['file_sha256'] as String?;
          transfer.chunkHashes =
              (data['chunk_hashes'] as List<dynamic>).map((e) => e as String).toList();
          transfer.offerSent = true;
          _log('INFO', 'hashing complete: transfer=${transfer.transferId} sha256=${transfer.fileSha256} chunks=${transfer.chunkHashes?.length} — sending FileOfferSend');
          final offerData = <String, dynamic>{
            'target_user_id': transfer.targetUserId,
            'transfer_id': transfer.transferId,
            'filename': transfer.filename,
            'size': transfer.fileSize,
            'file_sha256': transfer.fileSha256,
            'chunk_hashes': transfer.chunkHashes,
          };
          if (transfer.folderId != null) {
            offerData['folder_id'] = transfer.folderId;
          }
          _gateway.send({
            'type': 'FileOfferSend',
            'data': offerData,
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

        // Start next queued upload
        if (_activeUploadTransferId == transfer.transferId) {
          _processQueue();
        }
      }

      // Also advance queue on upload error/cancel
      if ((transfer.state == TransferState.error || transfer.state == TransferState.cancelled) &&
          transfer.isUpload &&
          _activeUploadTransferId == transfer.transferId) {
        _processQueue();
      }

      // Check if parent folder is complete
      if (transfer.state == TransferState.complete && transfer.folderId != null) {
        _checkFolderCompletion(transfer.folderId!);
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

  void _checkFolderCompletion(String folderId) {
    final folder = _folders[folderId];
    if (folder == null || folder.state == FolderTransferState.complete) return;

    final allDone = folder.transferIdToPath.keys.every((tid) {
      final t = _transfers[tid];
      return t != null && t.state == TransferState.complete;
    });

    if (allDone && folder.transferIdToPath.isNotEmpty) {
      folder.state = FolderTransferState.complete;
      _log('INFO', 'folder complete: folder=$folderId name=${folder.folderName}');
    }
  }

  // ── Gateway event handlers ───────────────────────────────────────────

  void _handleFolderOffer(Map<String, dynamic> event) {
    final data = event['data'] as Map<String, dynamic>;
    final folderId = data['folder_id'] as String;
    final folderName = data['folder_name'] as String;
    final totalSize = data['total_size'] as int;
    final fileCount = data['file_count'] as int;
    final fromUserId = data['from_user_id'] as String;
    final manifestRaw = data['manifest'] as List<dynamic>;

    final manifest = manifestRaw.map((e) {
      final m = e as Map<String, dynamic>;
      return FolderFileEntry(
        relativePath: m['relative_path'] as String,
        size: m['size'] as int,
      );
    }).toList();

    _log('INFO', '_handleFolderOffer: folder=$folderId name=$folderName files=$fileCount size=$totalSize from=$fromUserId');

    final folder = FolderTransfer(
      folderId: folderId,
      folderName: folderName,
      totalSize: totalSize,
      fileCount: fileCount,
      isUpload: false,
      manifest: manifest,
      fromUserId: fromUserId,
    );
    _folders[folderId] = folder;
    onProgressUpdate?.call();
  }

  void _handleFolderAccept(Map<String, dynamic> event) {
    final data = event['data'] as Map<String, dynamic>;
    final folderId = data['folder_id'] as String;
    _log('INFO', '_handleFolderAccept: folder=$folderId — receiver accepted');
    onProgressUpdate?.call();
  }

  void _handleFolderReject(Map<String, dynamic> event) {
    final data = event['data'] as Map<String, dynamic>;
    final folderId = data['folder_id'] as String;
    _log('INFO', '_handleFolderReject: folder=$folderId — receiver rejected');

    final folder = _folders[folderId];
    if (folder != null) {
      folder.state = FolderTransferState.rejected;
      // Cancel all child transfers
      for (final tid in folder.transferIdToPath.keys) {
        cancelTransfer(tid);
      }
    }
    onProgressUpdate?.call();
  }

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
    final folderId = data['folder_id'] as String?;

    _log('INFO', '_handleFileOffer: transfer=$transferId file=$filename size=$size from=$fromUserId folder=$folderId sha256=$fileSha256 chunks=${chunkHashes?.length}');

    final transfer = FileTransfer(
      transferId: transferId,
      filename: filename,
      fileSize: size,
      isUpload: false,
      fromUserId: fromUserId,
      fileServerUrl: fileServerUrl,
      fileSha256: fileSha256,
      chunkHashes: chunkHashes,
      folderId: folderId,
    );
    _transfers[transferId] = transfer;

    // If this file belongs to a folder, register it as a child
    if (folderId != null) {
      final folder = _folders[folderId];
      if (folder != null) {
        folder.transferIdToPath[transferId] = filename;

        // If folder is already accepted, auto-accept this file
        if (folder.state == FolderTransferState.active &&
            folder.saveDir != null &&
            folder.masterKey != null &&
            folder.salt != null) {
          final savePath = '${folder.saveDir}/$filename'.replaceAll('/', '\\');
          _log('INFO', '_handleFileOffer: auto-accepting folder child transfer=$transferId savePath=$savePath');
          acceptOffer(transferId, savePath, folder.masterKey!, folder.salt!);
          return; // Don't trigger UI update for individual pending offer
        }
      }
    }

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

  void _handleFastProgress(Map<String, dynamic> event) {
    final data = event['data'] as Map<String, dynamic>;
    final transferId = data['transfer_id'] as String;
    final bytesDone = data['bytes_done'] as int? ?? 0;
    final bytesTotal = data['bytes_total'] as int? ?? 0;

    final transfer = _transfers[transferId];
    if (transfer == null || transfer.isUpload) return;

    // Update receiver's view of sender's upload progress
    if (transfer.nativeHandle == null) {
      // Download hasn't started yet — show sender's upload progress
      transfer.bytesDone = bytesDone;
      transfer.bytesTotal = bytesTotal;
      onProgressUpdate?.call();
    }
  }
}

class _PendingDownload {
  final String savePath;
  final String masterKey;
  final String salt;
  _PendingDownload(this.savePath, this.masterKey, this.salt);
}

class _QueuedSend {
  final String transferId;
  final String filePath;
  final String masterKey;
  final String salt;
  _QueuedSend({
    required this.transferId,
    required this.filePath,
    required this.masterKey,
    required this.salt,
  });
}
