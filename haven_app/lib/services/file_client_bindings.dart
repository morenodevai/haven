import 'dart:ffi';
import 'dart:io';

import 'package:ffi/ffi.dart';

/// FFI bindings to the haven_file_client native DLL.
///
/// Provides upload/download/cancel/progress functions that delegate to the
/// Rust DLL for streaming HTTP file transfers with AES-256-GCM encryption.

// ── Native struct ────────────────────────────────────────────────────────

/// Matches the C repr struct TransferProgressResult from Rust FFI.
final class TransferProgressResult extends Struct {
  @Uint64()
  external int bytesDone;

  @Uint64()
  external int bytesTotal;

  @Uint8()
  external int state;
}

// ── Transfer state constants (match Rust) ────────────────────────────────

class TransferState {
  static const int idle = 0;
  static const int hashing = 1;
  static const int transferring = 2; // uploading or downloading
  static const int complete = 3;
  static const int error = 4;
  static const int cancelled = 5;
}

// ── FFI typedefs ─────────────────────────────────────────────────────────

typedef _UploadFileNative = Pointer<Void> Function(
  Pointer<Utf8> filePath,
  Pointer<Utf8> serverUrl,
  Pointer<Utf8> transferId,
  Pointer<Utf8> jwtToken,
  Pointer<Utf8> masterKey,
  Pointer<Utf8> salt,
);
typedef _UploadFileDart = Pointer<Void> Function(
  Pointer<Utf8> filePath,
  Pointer<Utf8> serverUrl,
  Pointer<Utf8> transferId,
  Pointer<Utf8> jwtToken,
  Pointer<Utf8> masterKey,
  Pointer<Utf8> salt,
);

typedef _DownloadFileNative = Pointer<Void> Function(
  Pointer<Utf8> savePath,
  Pointer<Utf8> serverUrl,
  Pointer<Utf8> transferId,
  Pointer<Utf8> jwtToken,
  Pointer<Utf8> masterKey,
  Pointer<Utf8> salt,
  Pointer<Utf8> fileSha256,
  Pointer<Utf8> chunkHashesJson,
);
typedef _DownloadFileDart = Pointer<Void> Function(
  Pointer<Utf8> savePath,
  Pointer<Utf8> serverUrl,
  Pointer<Utf8> transferId,
  Pointer<Utf8> jwtToken,
  Pointer<Utf8> masterKey,
  Pointer<Utf8> salt,
  Pointer<Utf8> fileSha256,
  Pointer<Utf8> chunkHashesJson,
);

typedef _CancelNative = Void Function(Pointer<Void> handle);
typedef _CancelDart = void Function(Pointer<Void> handle);

typedef _ProgressNative = TransferProgressResult Function(Pointer<Void> handle);
typedef _ProgressDart = TransferProgressResult Function(Pointer<Void> handle);

typedef _FreeNative = Void Function(Pointer<Void> handle);
typedef _FreeDart = void Function(Pointer<Void> handle);

typedef _GetHashesJsonNative = Pointer<Utf8> Function(Pointer<Void> handle);
typedef _GetHashesJsonDart = Pointer<Utf8> Function(Pointer<Void> handle);

typedef _FreeStringNative = Void Function(Pointer<Utf8> ptr);
typedef _FreeStringDart = void Function(Pointer<Utf8> ptr);

typedef _GetLastErrorNative = Pointer<Utf8> Function(Pointer<Void> handle);
typedef _GetLastErrorDart = Pointer<Utf8> Function(Pointer<Void> handle);

// ── Bindings class ───────────────────────────────────────────────────────

class FileClientBindings {
  late final _UploadFileDart _uploadFile;
  late final _DownloadFileDart _downloadFile;
  late final _CancelDart _cancel;
  late final _ProgressDart _progress;
  late final _FreeDart _free;
  late final _GetHashesJsonDart _getHashesJson;
  late final _FreeStringDart _freeString;
  late final _GetLastErrorDart _getLastError;

  static FileClientBindings? _instance;

  factory FileClientBindings() {
    _instance ??= FileClientBindings._init();
    return _instance!;
  }

  FileClientBindings._init() {
    final lib = _loadLibrary();

    _uploadFile = lib
        .lookup<NativeFunction<_UploadFileNative>>('haven_upload_file')
        .asFunction<_UploadFileDart>();

    _downloadFile = lib
        .lookup<NativeFunction<_DownloadFileNative>>('haven_download_file')
        .asFunction<_DownloadFileDart>();

    _cancel = lib
        .lookup<NativeFunction<_CancelNative>>('haven_transfer_cancel')
        .asFunction<_CancelDart>();

    _progress = lib
        .lookup<NativeFunction<_ProgressNative>>('haven_transfer_progress')
        .asFunction<_ProgressDart>();

    _free = lib
        .lookup<NativeFunction<_FreeNative>>('haven_transfer_free')
        .asFunction<_FreeDart>();

    _getHashesJson = lib
        .lookup<NativeFunction<_GetHashesJsonNative>>('haven_upload_hashes_json')
        .asFunction<_GetHashesJsonDart>();

    _freeString = lib
        .lookup<NativeFunction<_FreeStringNative>>('haven_free_string')
        .asFunction<_FreeStringDart>();

    _getLastError = lib
        .lookup<NativeFunction<_GetLastErrorNative>>('haven_get_last_error')
        .asFunction<_GetLastErrorDart>();
  }

  static DynamicLibrary _loadLibrary() {
    if (Platform.isWindows) {
      return DynamicLibrary.open('haven_file_client.dll');
    } else if (Platform.isLinux) {
      return DynamicLibrary.open('libhaven_file_client.so');
    } else if (Platform.isMacOS) {
      return DynamicLibrary.open('libhaven_file_client.dylib');
    } else {
      throw UnsupportedError('Unsupported platform for file client');
    }
  }

  /// Start an upload. Returns a native handle pointer.
  Pointer<Void> uploadFile({
    required String filePath,
    required String serverUrl,
    required String transferId,
    required String jwtToken,
    required String masterKey,
    required String salt,
  }) {
    final pFilePath = filePath.toNativeUtf8();
    final pServerUrl = serverUrl.toNativeUtf8();
    final pTransferId = transferId.toNativeUtf8();
    final pJwtToken = jwtToken.toNativeUtf8();
    final pMasterKey = masterKey.toNativeUtf8();
    final pSalt = salt.toNativeUtf8();

    try {
      return _uploadFile(
        pFilePath, pServerUrl, pTransferId, pJwtToken, pMasterKey, pSalt,
      );
    } finally {
      calloc.free(pFilePath);
      calloc.free(pServerUrl);
      calloc.free(pTransferId);
      calloc.free(pJwtToken);
      calloc.free(pMasterKey);
      calloc.free(pSalt);
    }
  }

  /// Start a download. Returns a native handle pointer.
  Pointer<Void> downloadFile({
    required String savePath,
    required String serverUrl,
    required String transferId,
    required String jwtToken,
    required String masterKey,
    required String salt,
    required String fileSha256,
    required String chunkHashesJson,
  }) {
    final pSavePath = savePath.toNativeUtf8();
    final pServerUrl = serverUrl.toNativeUtf8();
    final pTransferId = transferId.toNativeUtf8();
    final pJwtToken = jwtToken.toNativeUtf8();
    final pMasterKey = masterKey.toNativeUtf8();
    final pSalt = salt.toNativeUtf8();
    final pFileSha256 = fileSha256.toNativeUtf8();
    final pChunkHashes = chunkHashesJson.toNativeUtf8();

    try {
      return _downloadFile(
        pSavePath, pServerUrl, pTransferId, pJwtToken, pMasterKey, pSalt,
        pFileSha256, pChunkHashes,
      );
    } finally {
      calloc.free(pSavePath);
      calloc.free(pServerUrl);
      calloc.free(pTransferId);
      calloc.free(pJwtToken);
      calloc.free(pMasterKey);
      calloc.free(pSalt);
      calloc.free(pFileSha256);
      calloc.free(pChunkHashes);
    }
  }

  /// Cancel a transfer.
  void cancel(Pointer<Void> handle) => _cancel(handle);

  /// Poll transfer progress.
  TransferProgressResult getProgress(Pointer<Void> handle) => _progress(handle);

  /// Free a transfer handle. Must be called when done.
  void free(Pointer<Void> handle) => _free(handle);

  /// Returns the upload hashes JSON `{"file_sha256":"...","chunk_hashes":[...]}` once
  /// pass 1 (hashing) is complete, or null if not ready yet.
  String? getUploadHashesJson(Pointer<Void> handle) {
    final ptr = _getHashesJson(handle);
    if (ptr == nullptr) return null;
    try {
      return ptr.toDartString();
    } finally {
      _freeString(ptr);
    }
  }

  /// Returns the last error message from the native transfer, or null if no error.
  String? getLastError(Pointer<Void> handle) {
    final ptr = _getLastError(handle);
    if (ptr == nullptr) return null;
    try {
      return ptr.toDartString();
    } finally {
      _freeString(ptr);
    }
  }
}
