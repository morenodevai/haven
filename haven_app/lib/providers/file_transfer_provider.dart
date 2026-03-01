import 'package:flutter_riverpod/flutter_riverpod.dart';

import 'package:haven_app/services/file_transfer_service.dart';

/// State for file transfers UI.
class FileTransferState {
  final Map<String, FileTransfer> transfers;
  final Map<String, FolderTransfer> folders;

  const FileTransferState({
    this.transfers = const {},
    this.folders = const {},
  });

  FileTransferState copyWith({
    Map<String, FileTransfer>? transfers,
    Map<String, FolderTransfer>? folders,
  }) {
    return FileTransferState(
      transfers: transfers ?? this.transfers,
      folders: folders ?? this.folders,
    );
  }

  /// Incoming individual file offers (not part of a folder).
  List<FileTransfer> get pendingOffers => transfers.values
      .where((t) =>
          !t.isUpload &&
          t.nativeHandle == null &&
          t.state == 0 &&
          t.folderId == null)
      .toList();

  /// Active individual transfers (not part of a folder).
  List<FileTransfer> get active => transfers.values
      .where((t) =>
          (t.state == 1 || t.state == 2) &&
          t.folderId == null)
      .toList();

  /// Completed individual transfers (not part of a folder).
  List<FileTransfer> get completed => transfers.values
      .where((t) => t.state == 3 && t.folderId == null)
      .toList();

  /// Failed/cancelled individual transfers (not part of a folder).
  List<FileTransfer> get failed => transfers.values
      .where((t) => (t.state == 4 || t.state == 5) && t.folderId == null)
      .toList();

  /// Pending folder offers waiting for user action.
  List<FolderTransfer> get pendingFolderOffers => folders.values
      .where((f) => !f.isUpload && f.state == FolderTransferState.pending)
      .toList();

  /// Active folder transfers (uploading or downloading).
  List<FolderTransfer> get activeFolders => folders.values
      .where((f) => f.state == FolderTransferState.active)
      .toList();

  /// Completed folder transfers.
  List<FolderTransfer> get completedFolders => folders.values
      .where((f) => f.state == FolderTransferState.complete)
      .toList();
}

class FileTransferNotifier extends StateNotifier<FileTransferState> {
  FileTransferService? _service;

  FileTransferNotifier() : super(const FileTransferState());

  void init(FileTransferService service) {
    _service = service;
    _service!.onProgressUpdate = _syncState;
  }

  void _syncState() {
    if (_service == null) return;
    state = state.copyWith(
      transfers: Map.from(_service!.transfers),
      folders: Map.from(_service!.folders),
    );
  }

  /// Send a file to a peer.
  String? sendFile({
    required String filePath,
    required String filename,
    required int fileSize,
    required String targetUserId,
    required String masterKey,
    required String salt,
  }) {
    if (_service == null) return null;
    final id = _service!.offerFile(
      filePath: filePath,
      filename: filename,
      fileSize: fileSize,
      targetUserId: targetUserId,
      masterKey: masterKey,
      salt: salt,
    );
    _syncState();
    return id;
  }

  /// Accept an incoming file offer.
  void acceptOffer(String transferId, String savePath, String masterKey, String salt) {
    _service?.acceptOffer(transferId, savePath, masterKey, salt);
    _syncState();
  }

  /// Reject an incoming file offer.
  void rejectOffer(String transferId) {
    _service?.rejectOffer(transferId);
    _syncState();
  }

  /// Cancel a transfer.
  void cancelTransfer(String transferId) {
    _service?.cancelTransfer(transferId);
    _syncState();
  }

  /// Send a folder to a peer. Returns the folder ID.
  String? sendFolder({
    required String folderPath,
    required String targetUserId,
    required String masterKey,
    required String salt,
  }) {
    if (_service == null) return null;
    final id = _service!.offerFolder(
      folderPath: folderPath,
      targetUserId: targetUserId,
      masterKey: masterKey,
      salt: salt,
    );
    _syncState();
    return id;
  }

  /// Accept a folder offer.
  void acceptFolder(String folderId, String saveDir, String masterKey, String salt) {
    _service?.acceptFolder(folderId, saveDir, masterKey, salt);
    _syncState();
  }

  /// Reject a folder offer.
  void rejectFolder(String folderId) {
    _service?.rejectFolder(folderId);
    _syncState();
  }

  /// Cancel a folder transfer and remove it + all children immediately.
  void cancelAndRemoveFolder(String folderId) {
    _service?.cancelAndRemoveFolder(folderId);
    _syncState();
  }

  /// Remove a completed/rejected folder.
  void removeFolder(String folderId) {
    _service?.removeFolder(folderId);
    _syncState();
  }

  /// Cancel a transfer and remove it immediately.
  void cancelAndRemoveTransfer(String transferId) {
    _service?.cancelAndRemoveTransfer(transferId);
    _syncState();
  }

  /// Accept all pending individual offers at once, saving to a directory.
  void acceptAll(String saveDir, String masterKey, String salt) {
    if (_service == null) return;
    for (final t in state.pendingOffers) {
      final safeName = t.filename.replaceAll('/', '\\');
      final savePath = '$saveDir\\$safeName';
      _service!.acceptOffer(t.transferId, savePath, masterKey, salt);
    }
    _syncState();
  }

  /// Remove a completed/cancelled transfer from the list.
  void removeTransfer(String transferId) {
    _service?.removeTransfer(transferId);
    _syncState();
  }

  @override
  void dispose() {
    _service?.dispose();
    super.dispose();
  }
}

final fileTransferProvider =
    StateNotifierProvider<FileTransferNotifier, FileTransferState>((ref) {
  return FileTransferNotifier();
});
