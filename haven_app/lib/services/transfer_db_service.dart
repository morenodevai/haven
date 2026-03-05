import 'dart:io';

import 'package:path_provider/path_provider.dart';
import 'package:sqflite_common_ffi/sqflite_ffi.dart';

/// Persists file transfer state to local SQLite so transfers survive app restarts.
class TransferDbService {
  Database? _db;

  Future<void> init() async {
    sqfliteFfiInit();
    final dbFactory = databaseFactoryFfi;
    final dir = await getApplicationSupportDirectory();
    final dbPath = '${dir.path}${Platform.pathSeparator}haven_transfers.db';
    _db = await dbFactory.openDatabase(
      dbPath,
      options: OpenDatabaseOptions(
        version: 1,
        onCreate: (db, version) async {
          await db.execute('''
            CREATE TABLE transfers (
              transfer_id TEXT PRIMARY KEY,
              filename TEXT NOT NULL,
              file_size INTEGER NOT NULL,
              is_upload INTEGER NOT NULL,
              target_user_id TEXT,
              from_user_id TEXT,
              file_path TEXT,
              save_path TEXT,
              file_server_url TEXT,
              file_sha256 TEXT,
              chunk_hashes TEXT,
              folder_id TEXT,
              master_key TEXT,
              salt TEXT,
              bytes_done INTEGER DEFAULT 0,
              state INTEGER DEFAULT 0,
              created_at TEXT NOT NULL
            )
          ''');
        },
      ),
    );
  }

  Future<void> upsertTransfer(TransferRecord t) async {
    if (_db == null) return;
    await _db!.insert(
      'transfers',
      t.toMap(),
      conflictAlgorithm: ConflictAlgorithm.replace,
    );
  }

  Future<void> updateProgress(String transferId, int bytesDone, int state) async {
    if (_db == null) return;
    await _db!.update(
      'transfers',
      {'bytes_done': bytesDone, 'state': state},
      where: 'transfer_id = ?',
      whereArgs: [transferId],
    );
  }

  Future<void> markComplete(String transferId) async {
    if (_db == null) return;
    await _db!.update(
      'transfers',
      {'state': 3}, // TransferState.complete
      where: 'transfer_id = ?',
      whereArgs: [transferId],
    );
  }

  Future<void> deleteTransfer(String transferId) async {
    if (_db == null) return;
    await _db!.delete(
      'transfers',
      where: 'transfer_id = ?',
      whereArgs: [transferId],
    );
  }

  /// Returns all incomplete transfers (not complete, not cancelled).
  Future<List<TransferRecord>> getIncompleteTransfers() async {
    if (_db == null) return [];
    final rows = await _db!.query(
      'transfers',
      where: 'state < 3', // idle, hashing, transferring
    );
    return rows.map(TransferRecord.fromMap).toList();
  }

  Future<void> close() async {
    await _db?.close();
    _db = null;
  }
}

/// Serializable transfer record for local persistence.
class TransferRecord {
  final String transferId;
  final String filename;
  final int fileSize;
  final bool isUpload;
  final String? targetUserId;
  final String? fromUserId;
  final String? filePath;
  final String? savePath;
  final String? fileServerUrl;
  final String? fileSha256;
  final String? chunkHashes; // JSON string
  final String? folderId;
  final String? masterKey;
  final String? salt;
  final int bytesDone;
  final int state;
  final String createdAt;

  TransferRecord({
    required this.transferId,
    required this.filename,
    required this.fileSize,
    required this.isUpload,
    this.targetUserId,
    this.fromUserId,
    this.filePath,
    this.savePath,
    this.fileServerUrl,
    this.fileSha256,
    this.chunkHashes,
    this.folderId,
    this.masterKey,
    this.salt,
    this.bytesDone = 0,
    this.state = 0,
    required this.createdAt,
  });

  Map<String, dynamic> toMap() => {
        'transfer_id': transferId,
        'filename': filename,
        'file_size': fileSize,
        'is_upload': isUpload ? 1 : 0,
        'target_user_id': targetUserId,
        'from_user_id': fromUserId,
        'file_path': filePath,
        'save_path': savePath,
        'file_server_url': fileServerUrl,
        'file_sha256': fileSha256,
        'chunk_hashes': chunkHashes,
        'folder_id': folderId,
        'master_key': masterKey,
        'salt': salt,
        'bytes_done': bytesDone,
        'state': state,
        'created_at': createdAt,
      };

  static TransferRecord fromMap(Map<String, dynamic> m) => TransferRecord(
        transferId: m['transfer_id'] as String,
        filename: m['filename'] as String,
        fileSize: m['file_size'] as int,
        isUpload: (m['is_upload'] as int) == 1,
        targetUserId: m['target_user_id'] as String?,
        fromUserId: m['from_user_id'] as String?,
        filePath: m['file_path'] as String?,
        savePath: m['save_path'] as String?,
        fileServerUrl: m['file_server_url'] as String?,
        fileSha256: m['file_sha256'] as String?,
        chunkHashes: m['chunk_hashes'] as String?,
        folderId: m['folder_id'] as String?,
        masterKey: m['master_key'] as String?,
        salt: m['salt'] as String?,
        bytesDone: m['bytes_done'] as int? ?? 0,
        state: m['state'] as int? ?? 0,
        createdAt: m['created_at'] as String,
      );
}
