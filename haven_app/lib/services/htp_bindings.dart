// Dart FFI bindings to the haven-transfer native library.
//
// Provides high-speed encrypted UDP file transfer via:
//   - htp_send_file: start sending a file
//   - htp_recv_file: start receiving a file
///   - htp_sender_nack: feed NACK data to sender
///   - htp_sender_rtt: feed RTT measurement to sender
///   - htp_sender_ack: acknowledge received packets
///   - htp_cancel: cancel a transfer
///   - htp_progress: get transfer progress (0.0 - 1.0)
///   - htp_stats: get detailed transfer stats
///   - htp_chunk_size: get max plaintext per packet
///   - htp_random_salt: generate random 32-byte salt

import 'dart:ffi';
import 'dart:io';
import 'package:ffi/ffi.dart';

// ── Native function typedefs ──

// u32 htp_send_file(char* file_path, char* dest_addr, u8* master_key, u8* salt, char* jwt_token)
typedef HtpSendFileNative = Uint32 Function(
    Pointer<Utf8> filePath,
    Pointer<Utf8> destAddr,
    Pointer<Uint8> masterKey,
    Pointer<Uint8> salt,
    Pointer<Utf8> jwtToken);
typedef HtpSendFileDart = int Function(
    Pointer<Utf8> filePath,
    Pointer<Utf8> destAddr,
    Pointer<Uint8> masterKey,
    Pointer<Uint8> salt,
    Pointer<Utf8> jwtToken);

// u32 htp_recv_file(u32 session_id, char* output_path, u64 file_size, u64 total_chunks,
//                   char* listen_addr, u8* master_key, u8* salt,
//                   char* jwt_token, char* relay_addr,
//                   callback nack_cb, callback done_cb)
typedef HtpRecvFileNative = Uint32 Function(
    Uint32 sessionId,
    Pointer<Utf8> outputPath,
    Uint64 fileSize,
    Uint64 totalChunks,
    Pointer<Utf8> listenAddr,
    Pointer<Uint8> masterKey,
    Pointer<Uint8> salt,
    Pointer<Utf8> jwtToken,
    Pointer<Utf8> relayAddr,
    Pointer<NativeFunction<NackCallbackNative>> nackCallback,
    Pointer<NativeFunction<DoneCallbackNative>> doneCallback);
typedef HtpRecvFileDart = int Function(
    int sessionId,
    Pointer<Utf8> outputPath,
    int fileSize,
    int totalChunks,
    Pointer<Utf8> listenAddr,
    Pointer<Uint8> masterKey,
    Pointer<Uint8> salt,
    Pointer<Utf8> jwtToken,
    Pointer<Utf8> relayAddr,
    Pointer<NativeFunction<NackCallbackNative>> nackCallback,
    Pointer<NativeFunction<DoneCallbackNative>> doneCallback);

// Callback types
typedef NackCallbackNative = Void Function(
    Uint32 sessionId, Pointer<Uint64> missing, Uint32 count);
typedef DoneCallbackNative = Void Function(Uint32 sessionId, Uint64 totalBytes);

// void htp_sender_nack(u32 session_id, u64* missing, u32 count)
typedef HtpSenderNackNative = Void Function(
    Uint32 sessionId, Pointer<Uint64> missing, Uint32 count);
typedef HtpSenderNackDart = void Function(
    int sessionId, Pointer<Uint64> missing, int count);

// void htp_sender_rtt(u32 session_id, u64 rtt_microseconds)
typedef HtpSenderRttNative = Void Function(
    Uint32 sessionId, Uint64 rttMicroseconds);
typedef HtpSenderRttDart = void Function(int sessionId, int rttMicroseconds);

// void htp_sender_ack(u32 session_id, u64 up_to_sequence)
typedef HtpSenderAckNative = Void Function(
    Uint32 sessionId, Uint64 upToSequence);
typedef HtpSenderAckDart = void Function(int sessionId, int upToSequence);

// void htp_cancel(u32 session_id)
typedef HtpCancelNative = Void Function(Uint32 sessionId);
typedef HtpCancelDart = void Function(int sessionId);

// f64 htp_progress(u32 session_id)
typedef HtpProgressNative = Double Function(Uint32 sessionId);
typedef HtpProgressDart = double Function(int sessionId);

// bool htp_stats(u32 session_id, u64* bytes, u64* total, u64* rate, u64* retransmits)
typedef HtpStatsNative = Bool Function(
    Uint32 sessionId,
    Pointer<Uint64> bytesTransferred,
    Pointer<Uint64> totalBytes,
    Pointer<Uint64> rateBps,
    Pointer<Uint64> retransmits);
typedef HtpStatsDart = bool Function(
    int sessionId,
    Pointer<Uint64> bytesTransferred,
    Pointer<Uint64> totalBytes,
    Pointer<Uint64> rateBps,
    Pointer<Uint64> retransmits);

// u32 htp_chunk_size()
typedef HtpChunkSizeNative = Uint32 Function();
typedef HtpChunkSizeDart = int Function();

// void htp_random_salt(u8* out)
typedef HtpRandomSaltNative = Void Function(Pointer<Uint8> out);
typedef HtpRandomSaltDart = void Function(Pointer<Uint8> out);

// ── Transfer stats result ──

class TransferStatsResult {
  final int bytesTransferred;
  final int totalBytes;
  final int rateBps;
  final int retransmits;

  TransferStatsResult({
    required this.bytesTransferred,
    required this.totalBytes,
    required this.rateBps,
    required this.retransmits,
  });

  double get progress =>
      totalBytes > 0 ? bytesTransferred / totalBytes : 0.0;

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
}

// ── HTP Bindings ──

class HtpBindings {
  late final DynamicLibrary _lib;
  late final HtpSendFileDart sendFile;
  late final HtpRecvFileDart recvFile;
  late final HtpSenderNackDart senderNack;
  late final HtpSenderRttDart senderRtt;
  late final HtpSenderAckDart senderAck;
  late final HtpCancelDart cancel;
  late final HtpProgressDart progress;
  late final HtpStatsDart _stats;
  late final HtpChunkSizeDart chunkSize;
  late final HtpRandomSaltDart _randomSalt;

  static HtpBindings? _instance;

  factory HtpBindings() {
    _instance ??= HtpBindings._init();
    return _instance!;
  }

  HtpBindings._init() {
    // Load the native library from next to the executable
    final exeDir = File(Platform.resolvedExecutable).parent.path;
    final libPath = '$exeDir/haven_transfer.dll';
    _lib = DynamicLibrary.open(libPath);

    sendFile = _lib
        .lookupFunction<HtpSendFileNative, HtpSendFileDart>('htp_send_file');
    recvFile = _lib.lookupFunction<HtpRecvFileNative, HtpRecvFileDart>(
        'htp_recv_file');
    senderNack = _lib.lookupFunction<HtpSenderNackNative, HtpSenderNackDart>(
        'htp_sender_nack');
    senderRtt = _lib.lookupFunction<HtpSenderRttNative, HtpSenderRttDart>(
        'htp_sender_rtt');
    senderAck = _lib.lookupFunction<HtpSenderAckNative, HtpSenderAckDart>(
        'htp_sender_ack');
    cancel =
        _lib.lookupFunction<HtpCancelNative, HtpCancelDart>('htp_cancel');
    progress = _lib
        .lookupFunction<HtpProgressNative, HtpProgressDart>('htp_progress');
    _stats =
        _lib.lookupFunction<HtpStatsNative, HtpStatsDart>('htp_stats');
    chunkSize = _lib.lookupFunction<HtpChunkSizeNative, HtpChunkSizeDart>(
        'htp_chunk_size');
    _randomSalt = _lib.lookupFunction<HtpRandomSaltNative, HtpRandomSaltDart>(
        'htp_random_salt');
  }

  /// Get detailed transfer stats. Returns null if session not found.
  TransferStatsResult? getStats(int sessionId) {
    final pBytes = calloc<Uint64>();
    final pTotal = calloc<Uint64>();
    final pRate = calloc<Uint64>();
    final pRetransmits = calloc<Uint64>();

    try {
      final found = _stats(sessionId, pBytes, pTotal, pRate, pRetransmits);
      if (!found) return null;

      return TransferStatsResult(
        bytesTransferred: pBytes.value,
        totalBytes: pTotal.value,
        rateBps: pRate.value,
        retransmits: pRetransmits.value,
      );
    } finally {
      calloc.free(pBytes);
      calloc.free(pTotal);
      calloc.free(pRate);
      calloc.free(pRetransmits);
    }
  }

  /// Generate a random 32-byte salt.
  List<int> randomSalt() {
    final pSalt = calloc<Uint8>(32);
    try {
      _randomSalt(pSalt);
      return List<int>.generate(32, (i) => pSalt[i]);
    } finally {
      calloc.free(pSalt);
    }
  }
}
