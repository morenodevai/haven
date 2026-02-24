import 'package:flutter_riverpod/flutter_riverpod.dart';

import 'package:haven_app/providers/auth_provider.dart';
import 'package:haven_app/providers/gateway_provider.dart';
import 'package:haven_app/services/file_transfer_service.dart';

/// State for file transfers UI.
class FileTransferState {
  final Map<String, FileTransfer> transfers;

  const FileTransferState({this.transfers = const {}});

  FileTransferState copyWith({Map<String, FileTransfer>? transfers}) {
    return FileTransferState(transfers: transfers ?? this.transfers);
  }

  /// Incoming offers waiting for user action.
  List<FileTransfer> get pendingOffers => transfers.values
      .where((t) => !t.isUpload && t.nativeHandle == null && t.state == 0)
      .toList();

  /// Active transfers (uploading or downloading).
  List<FileTransfer> get active => transfers.values
      .where((t) => t.state == 1 || t.state == 2)
      .toList();

  /// Completed transfers.
  List<FileTransfer> get completed => transfers.values
      .where((t) => t.state == 3)
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
    state = state.copyWith(transfers: Map.from(_service!.transfers));
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
