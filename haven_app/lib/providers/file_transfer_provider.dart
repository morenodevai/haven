import 'dart:async';

import 'package:flutter_riverpod/flutter_riverpod.dart';

import 'package:haven_app/providers/auth_provider.dart';
import 'package:haven_app/providers/gateway_provider.dart';
import 'package:haven_app/services/htp_service.dart';

class FileTransferState {
  final Map<int, HtpTransfer> transfers;
  final Map<String, dynamic>? pendingOffer;
  final String? error;

  const FileTransferState({
    this.transfers = const {},
    this.pendingOffer,
    this.error,
  });

  FileTransferState copyWith({
    Map<int, HtpTransfer>? transfers,
    Map<String, dynamic>? pendingOffer,
    bool clearOffer = false,
    String? error,
    bool clearError = false,
  }) {
    return FileTransferState(
      transfers: transfers ?? this.transfers,
      pendingOffer: clearOffer ? null : (pendingOffer ?? this.pendingOffer),
      error: clearError ? null : (error ?? this.error),
    );
  }
}

class FileTransferNotifier extends StateNotifier<FileTransferState> {
  final Ref _ref;
  HtpService? _htpService;
  StreamSubscription<HtpTransfer>? _transferSub;
  StreamSubscription<Map<String, dynamic>>? _offerSub;

  FileTransferNotifier(this._ref) : super(const FileTransferState());

  /// Initialize the HTP service and start listening to streams.
  void init() {
    if (_htpService != null) return;

    final gateway = _ref.read(gatewayServiceProvider);
    final authService = _ref.read(authServiceProvider);

    _htpService = HtpService(gateway, authService);

    _transferSub = _htpService!.transferStream.listen((transfer) {
      final updated = Map<int, HtpTransfer>.from(state.transfers);
      updated[transfer.sessionId] = transfer;
      state = state.copyWith(transfers: updated);
    });

    _offerSub = _htpService!.offerStream.listen((offer) {
      state = state.copyWith(pendingOffer: offer);
    });
  }

  /// Send a file to a specific user.
  Future<void> sendFile(String filePath, String recipientId) async {
    state = state.copyWith(clearError: true);
    try {
      final transfer = await _htpService?.sendFile(
        filePath: filePath,
        recipientId: recipientId,
      );
      if (transfer == null) {
        state = state.copyWith(error: 'File not found');
      }
    } catch (e) {
      state = state.copyWith(error: 'Failed to send file: $e');
    }
  }

  /// Accept an incoming file transfer offer.
  Future<void> acceptOffer(String savePath) async {
    final offer = state.pendingOffer;
    if (offer == null) return;

    state = state.copyWith(clearOffer: true, clearError: true);
    try {
      await _htpService?.acceptOffer(offer, savePath);
    } catch (e) {
      state = state.copyWith(error: 'Failed to accept transfer: $e');
    }
  }

  /// Reject an incoming file transfer offer.
  void rejectOffer() {
    final offer = state.pendingOffer;
    if (offer == null) return;

    _htpService?.rejectOffer(offer);
    state = state.copyWith(clearOffer: true);
  }

  /// Cancel an active transfer.
  void cancelTransfer(int sessionId) {
    _htpService?.cancelTransfer(sessionId);
  }

  /// Route a gateway control message to the HTP service.
  void handleControlMessage(Map<String, dynamic> msg) {
    _htpService?.handleControlMessage(msg);
  }

  @override
  void dispose() {
    _transferSub?.cancel();
    _offerSub?.cancel();
    _htpService?.dispose();
    super.dispose();
  }
}

final fileTransferProvider =
    StateNotifierProvider<FileTransferNotifier, FileTransferState>((ref) {
  return FileTransferNotifier(ref);
});
