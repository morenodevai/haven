import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:file_picker/file_picker.dart';
import 'package:path_provider/path_provider.dart';

import 'package:haven_app/config/constants.dart';
import 'package:haven_app/config/theme.dart';
import 'package:haven_app/providers/file_transfer_provider.dart';
import 'package:haven_app/providers/presence_provider.dart';
import 'package:haven_app/services/file_client_bindings.dart';
import 'package:haven_app/services/file_transfer_service.dart';

// Phase colors
const _colorQueued = Colors.grey;
const _colorHashing = Colors.amber;
const _colorUploading = Colors.blue;
const _colorDownloading = Colors.cyan;
const _colorComplete = Colors.green;
const _colorError = Colors.red;

class FileTransferScreen extends ConsumerWidget {
  const FileTransferScreen({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final transferState = ref.watch(fileTransferProvider);
    final onlineUsers = ref.watch(presenceProvider);

    final hasAnything = transferState.transfers.isNotEmpty ||
        transferState.folders.isNotEmpty;

    return Column(
      children: [
        // Header bar
        Container(
          padding: const EdgeInsets.all(16),
          decoration: const BoxDecoration(
            color: HavenTheme.surface,
            border: Border(
              bottom: BorderSide(color: HavenTheme.divider),
            ),
          ),
          child: Row(
            children: [
              const Icon(Icons.swap_horiz, color: HavenTheme.primaryLight),
              const SizedBox(width: 8),
              const Text(
                'File Transfers',
                style: TextStyle(
                  fontSize: 16,
                  fontWeight: FontWeight.w600,
                  color: HavenTheme.textPrimary,
                ),
              ),
              const Spacer(),
              ElevatedButton.icon(
                onPressed: () => _showSendFolderDialog(context, ref, onlineUsers),
                icon: const Icon(Icons.folder, size: 18),
                label: const Text('Send Folder'),
                style: ElevatedButton.styleFrom(
                  padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
                  textStyle: const TextStyle(fontSize: 13),
                ),
              ),
              const SizedBox(width: 8),
              ElevatedButton.icon(
                onPressed: () => _showSendDialog(context, ref, onlineUsers),
                icon: const Icon(Icons.upload_file, size: 18),
                label: const Text('Send File'),
                style: ElevatedButton.styleFrom(
                  padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
                  textStyle: const TextStyle(fontSize: 13),
                ),
              ),
            ],
          ),
        ),

        Expanded(
          child: hasAnything
              ? ListView(
                  children: [
                    // Pending folder offers
                    if (transferState.pendingFolderOffers.isNotEmpty) ...[
                      _SectionHeader(label: 'INCOMING FOLDER OFFERS'),
                      ...transferState.pendingFolderOffers.map(
                        (f) => _PendingFolderOfferCard(folder: f),
                      ),
                    ],

                    // Pending individual offers
                    if (transferState.pendingOffers.isNotEmpty) ...[
                      _SectionHeader(
                        label: 'INCOMING OFFERS',
                        trailing: transferState.pendingOffers.length > 1
                            ? TextButton.icon(
                                onPressed: () => _acceptAll(context, ref),
                                icon: const Icon(Icons.done_all, size: 16),
                                label: Text('Accept All (${transferState.pendingOffers.length})'),
                                style: TextButton.styleFrom(
                                  textStyle: const TextStyle(fontSize: 12),
                                  padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
                                ),
                              )
                            : null,
                      ),
                      ...transferState.pendingOffers.map(
                        (t) => _PendingOfferTile(transfer: t),
                      ),
                    ],

                    // Active folders
                    if (transferState.activeFolders.isNotEmpty) ...[
                      _SectionHeader(label: 'ACTIVE FOLDER TRANSFERS'),
                      ...transferState.activeFolders.map(
                        (f) => _ActiveFolderCard(
                          folder: f,
                          allTransfers: transferState.transfers,
                        ),
                      ),
                    ],

                    // Active individual transfers
                    if (transferState.active.isNotEmpty) ...[
                      _SectionHeader(label: 'ACTIVE TRANSFERS'),
                      ...transferState.active.map(
                        (t) => _ActiveTransferTile(transfer: t),
                      ),
                    ],

                    // Completed folders
                    if (transferState.completedFolders.isNotEmpty) ...[
                      _SectionHeader(label: 'COMPLETED FOLDERS'),
                      ...transferState.completedFolders.map(
                        (f) => _CompletedFolderCard(
                          folder: f,
                          allTransfers: transferState.transfers,
                        ),
                      ),
                    ],

                    // Failed/cancelled individual transfers
                    if (transferState.failed.isNotEmpty) ...[
                      _SectionHeader(label: 'FAILED'),
                      ...transferState.failed.map(
                        (t) => _FailedTransferTile(transfer: t),
                      ),
                    ],

                    // Completed individual transfers
                    if (transferState.completed.isNotEmpty) ...[
                      _SectionHeader(label: 'COMPLETED'),
                      ...transferState.completed.map(
                        (t) => _CompletedTransferTile(transfer: t),
                      ),
                    ],
                  ],
                )
              : Center(
                  child: Column(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      Icon(Icons.folder_open, size: 64, color: HavenTheme.textMuted),
                      const SizedBox(height: 16),
                      Text(
                        'No file transfers',
                        style: TextStyle(fontSize: 16, color: HavenTheme.textMuted),
                      ),
                      const SizedBox(height: 8),
                      Text(
                        'Click "Send File" or "Send Folder" to transfer to another user.',
                        style: TextStyle(fontSize: 13, color: HavenTheme.textMuted),
                      ),
                    ],
                  ),
                ),
        ),
      ],
    );
  }

  void _showSendDialog(BuildContext context, WidgetRef ref, Map<String, dynamic> onlineUsers) async {
    final result = await FilePicker.platform.pickFiles();
    if (result == null || result.files.isEmpty) return;

    final file = result.files.first;
    if (file.path == null) return;

    if (!context.mounted) return;
    final targetUserId = await showDialog<String>(
      context: context,
      builder: (ctx) => _UserPickerDialog(onlineUsers: onlineUsers),
    );
    if (targetUserId == null) return;

    ref.read(fileTransferProvider.notifier).sendFile(
      filePath: file.path!,
      filename: file.name,
      fileSize: file.size,
      targetUserId: targetUserId,
      masterKey: HavenConstants.defaultChannelKey,
      salt: 'file-transfer',
    );
  }

  void _showSendFolderDialog(BuildContext context, WidgetRef ref, Map<String, dynamic> onlineUsers) async {
    final folderPath = await FilePicker.platform.getDirectoryPath(
      dialogTitle: 'Select folder to send',
    );
    if (folderPath == null) return;

    final dir = Directory(folderPath);
    final files = dir.listSync(recursive: true).whereType<File>().toList();
    if (files.isEmpty) return;

    if (!context.mounted) return;
    final targetUserId = await showDialog<String>(
      context: context,
      builder: (ctx) => _UserPickerDialog(onlineUsers: onlineUsers),
    );
    if (targetUserId == null) return;

    ref.read(fileTransferProvider.notifier).sendFolder(
      folderPath: folderPath,
      targetUserId: targetUserId,
      masterKey: HavenConstants.defaultChannelKey,
      salt: 'file-transfer',
    );
  }

  void _acceptAll(BuildContext context, WidgetRef ref) async {
    final saveDir = await FilePicker.platform.getDirectoryPath(
      dialogTitle: 'Save all files to...',
    );
    if (saveDir == null) return;

    ref.read(fileTransferProvider.notifier).acceptAll(
      saveDir,
      HavenConstants.defaultChannelKey,
      'file-transfer',
    );
  }
}

// ── Shared widgets ──────────────────────────────────────────────────────

class _SectionHeader extends StatelessWidget {
  final String label;
  final Widget? trailing;
  const _SectionHeader({required this.label, this.trailing});

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 16, 16, 8),
      child: Row(
        children: [
          Text(
            label,
            style: TextStyle(
              fontSize: 11,
              fontWeight: FontWeight.w600,
              color: HavenTheme.textMuted,
              letterSpacing: 1.2,
            ),
          ),
          const Spacer(),
          if (trailing != null) trailing!,
        ],
      ),
    );
  }
}

class _UserPickerDialog extends StatelessWidget {
  final Map<String, dynamic> onlineUsers;
  const _UserPickerDialog({required this.onlineUsers});

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      backgroundColor: HavenTheme.surface,
      title: const Text('Send to...'),
      content: SizedBox(
        width: 300,
        child: ListView(
          shrinkWrap: true,
          children: onlineUsers.entries.map((entry) {
            final username = entry.value.username;
            return ListTile(
              leading: CircleAvatar(
                backgroundColor: HavenTheme.primaryLight,
                child: Text(
                  username[0].toUpperCase(),
                  style: const TextStyle(color: Colors.white),
                ),
              ),
              title: Text(
                username,
                style: const TextStyle(color: HavenTheme.textPrimary),
              ),
              onTap: () => Navigator.of(context).pop(entry.key),
            );
          }).toList(),
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('Cancel'),
        ),
      ],
    );
  }
}

String _formatBytes(int bytes) {
  if (bytes < 1024) return '$bytes B';
  if (bytes < 1024 * 1024) return '${(bytes / 1024).toStringAsFixed(1)} KB';
  if (bytes < 1024 * 1024 * 1024) {
    return '${(bytes / (1024 * 1024)).toStringAsFixed(1)} MB';
  }
  return '${(bytes / (1024 * 1024 * 1024)).toStringAsFixed(2)} GB';
}

/// Returns the phase color for a file transfer state.
Color _phaseColor(int state, bool isUpload) {
  switch (state) {
    case TransferState.idle:
      return _colorQueued;
    case TransferState.hashing:
      return _colorHashing;
    case TransferState.transferring:
      return isUpload ? _colorUploading : _colorDownloading;
    case TransferState.complete:
      return _colorComplete;
    case TransferState.error:
    case TransferState.cancelled:
      return _colorError;
    default:
      return _colorQueued;
  }
}

// ── Individual file tiles (unchanged visual, now with phase coloring) ───

class _PendingOfferTile extends ConsumerWidget {
  final FileTransfer transfer;
  const _PendingOfferTile({required this.transfer});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    return Card(
      margin: const EdgeInsets.symmetric(horizontal: 16, vertical: 4),
      color: HavenTheme.surfaceVariant,
      child: Padding(
        padding: const EdgeInsets.all(12),
        child: Row(
          children: [
            const Icon(Icons.file_present, color: HavenTheme.primaryLight),
            const SizedBox(width: 12),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    transfer.filename,
                    style: const TextStyle(
                      color: HavenTheme.textPrimary,
                      fontWeight: FontWeight.w500,
                    ),
                  ),
                  Text(
                    _formatBytes(transfer.fileSize),
                    style: TextStyle(fontSize: 12, color: HavenTheme.textMuted),
                  ),
                ],
              ),
            ),
            IconButton(
              icon: const Icon(Icons.check_circle, color: HavenTheme.online),
              onPressed: () async {
                final initialDir = (await getDownloadsDirectory() ?? await getApplicationDocumentsDirectory()).path;
                final savePath = await FilePicker.platform.saveFile(
                  dialogTitle: 'Save ${transfer.filename}',
                  fileName: transfer.filename,
                  initialDirectory: initialDir,
                );
                if (savePath == null) return;
                ref.read(fileTransferProvider.notifier).acceptOffer(
                  transfer.transferId,
                  savePath,
                  HavenConstants.defaultChannelKey,
                  'file-transfer',
                );
              },
              tooltip: 'Accept',
            ),
            IconButton(
              icon: const Icon(Icons.cancel, color: HavenTheme.error),
              onPressed: () {
                ref.read(fileTransferProvider.notifier).rejectOffer(transfer.transferId);
              },
              tooltip: 'Reject',
            ),
          ],
        ),
      ),
    );
  }
}

class _ActiveTransferTile extends ConsumerWidget {
  final FileTransfer transfer;
  const _ActiveTransferTile({required this.transfer});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final color = _phaseColor(transfer.state, transfer.isUpload);
    return Card(
      margin: const EdgeInsets.symmetric(horizontal: 16, vertical: 4),
      color: HavenTheme.surface,
      child: Padding(
        padding: const EdgeInsets.all(12),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Icon(
                  transfer.isUpload ? Icons.upload : Icons.download,
                  color: color,
                  size: 20,
                ),
                const SizedBox(width: 8),
                Expanded(
                  child: Text(
                    transfer.filename,
                    style: const TextStyle(
                      color: HavenTheme.textPrimary,
                      fontWeight: FontWeight.w500,
                    ),
                    overflow: TextOverflow.ellipsis,
                  ),
                ),
                IconButton(
                  icon: const Icon(Icons.close, size: 18, color: HavenTheme.error),
                  onPressed: () {
                    ref.read(fileTransferProvider.notifier).cancelAndRemoveTransfer(transfer.transferId);
                  },
                  tooltip: 'Cancel & Remove',
                  constraints: const BoxConstraints(minWidth: 32, minHeight: 32),
                ),
              ],
            ),
            const SizedBox(height: 8),
            LinearProgressIndicator(
              value: transfer.progress,
              backgroundColor: HavenTheme.divider,
              valueColor: AlwaysStoppedAnimation(color),
            ),
            const SizedBox(height: 4),
            Row(
              mainAxisAlignment: MainAxisAlignment.spaceBetween,
              children: [
                Text(
                  transfer.statusText,
                  style: TextStyle(fontSize: 12, color: HavenTheme.textMuted),
                ),
                Text(
                  '${_formatBytes(transfer.bytesDone)} / ${_formatBytes(transfer.bytesTotal)}',
                  style: TextStyle(fontSize: 12, color: HavenTheme.textMuted),
                ),
              ],
            ),
          ],
        ),
      ),
    );
  }
}

class _CompletedTransferTile extends ConsumerWidget {
  final FileTransfer transfer;
  const _CompletedTransferTile({required this.transfer});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    return Card(
      margin: const EdgeInsets.symmetric(horizontal: 16, vertical: 4),
      color: HavenTheme.surface,
      child: Padding(
        padding: const EdgeInsets.all(12),
        child: Row(
          children: [
            const Icon(Icons.check_circle, color: HavenTheme.online, size: 20),
            const SizedBox(width: 8),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    transfer.filename,
                    style: const TextStyle(
                      color: HavenTheme.textPrimary,
                      fontWeight: FontWeight.w500,
                    ),
                  ),
                  Text(
                    '${transfer.isUpload ? "Sent" : "Received"} — ${_formatBytes(transfer.fileSize)}',
                    style: TextStyle(fontSize: 12, color: HavenTheme.textMuted),
                  ),
                ],
              ),
            ),
            IconButton(
              icon: const Icon(Icons.close, size: 16, color: HavenTheme.textMuted),
              onPressed: () {
                ref.read(fileTransferProvider.notifier).removeTransfer(transfer.transferId);
              },
              tooltip: 'Dismiss',
              constraints: const BoxConstraints(minWidth: 32, minHeight: 32),
            ),
          ],
        ),
      ),
    );
  }
}

class _FailedTransferTile extends ConsumerWidget {
  final FileTransfer transfer;
  const _FailedTransferTile({required this.transfer});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    return Card(
      margin: const EdgeInsets.symmetric(horizontal: 16, vertical: 4),
      color: HavenTheme.surface,
      child: Padding(
        padding: const EdgeInsets.all(12),
        child: Row(
          children: [
            const Icon(Icons.error, color: _colorError, size: 20),
            const SizedBox(width: 8),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    transfer.filename,
                    style: const TextStyle(
                      color: HavenTheme.textPrimary,
                      fontWeight: FontWeight.w500,
                    ),
                  ),
                  Text(
                    transfer.statusText,
                    style: const TextStyle(fontSize: 12, color: _colorError),
                  ),
                ],
              ),
            ),
            IconButton(
              icon: const Icon(Icons.close, size: 16, color: HavenTheme.textMuted),
              onPressed: () {
                ref.read(fileTransferProvider.notifier).cancelAndRemoveTransfer(transfer.transferId);
              },
              tooltip: 'Clear',
              constraints: const BoxConstraints(minWidth: 32, minHeight: 32),
            ),
          ],
        ),
      ),
    );
  }
}

// ── Folder cards ────────────────────────────────────────────────────────

class _PendingFolderOfferCard extends ConsumerWidget {
  final FolderTransfer folder;
  const _PendingFolderOfferCard({required this.folder});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    return Card(
      margin: const EdgeInsets.symmetric(horizontal: 16, vertical: 4),
      color: HavenTheme.surfaceVariant,
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            // Header
            Row(
              children: [
                const Icon(Icons.folder, color: _colorHashing, size: 28),
                const SizedBox(width: 12),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        folder.folderName,
                        style: const TextStyle(
                          color: HavenTheme.textPrimary,
                          fontWeight: FontWeight.bold,
                          fontSize: 15,
                        ),
                      ),
                      const SizedBox(height: 2),
                      Text(
                        '${folder.fileCount} files — ${_formatBytes(folder.totalSize)}',
                        style: TextStyle(fontSize: 12, color: HavenTheme.textMuted),
                      ),
                    ],
                  ),
                ),
              ],
            ),

            // Expandable file tree preview
            if (folder.manifest.isNotEmpty) ...[
              const SizedBox(height: 12),
              _FolderManifestPreview(manifest: folder.manifest),
            ],

            // Accept / Reject buttons
            const SizedBox(height: 16),
            Row(
              children: [
                Expanded(
                  child: ElevatedButton.icon(
                    onPressed: () async {
                      final saveDir = await FilePicker.platform.getDirectoryPath(
                        dialogTitle: 'Save "${folder.folderName}" to...',
                      );
                      if (saveDir == null) return;
                      ref.read(fileTransferProvider.notifier).acceptFolder(
                        folder.folderId,
                        '$saveDir\\${folder.folderName}',
                        HavenConstants.defaultChannelKey,
                        'file-transfer',
                      );
                    },
                    icon: const Icon(Icons.check, size: 18),
                    label: const Text('Accept'),
                    style: ElevatedButton.styleFrom(
                      backgroundColor: _colorComplete.withValues(alpha: 0.8),
                      padding: const EdgeInsets.symmetric(vertical: 10),
                      textStyle: const TextStyle(fontSize: 14, fontWeight: FontWeight.w600),
                    ),
                  ),
                ),
                const SizedBox(width: 12),
                OutlinedButton.icon(
                  onPressed: () {
                    ref.read(fileTransferProvider.notifier).rejectFolder(folder.folderId);
                  },
                  icon: const Icon(Icons.close, size: 18),
                  label: const Text('Reject'),
                  style: OutlinedButton.styleFrom(
                    foregroundColor: _colorError,
                    side: const BorderSide(color: _colorError),
                    padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 10),
                    textStyle: const TextStyle(fontSize: 14, fontWeight: FontWeight.w600),
                  ),
                ),
              ],
            ),
          ],
        ),
      ),
    );
  }
}

/// Expandable preview of folder manifest — shows directory tree.
class _FolderManifestPreview extends StatefulWidget {
  final List<FolderFileEntry> manifest;
  const _FolderManifestPreview({required this.manifest});

  @override
  State<_FolderManifestPreview> createState() => _FolderManifestPreviewState();
}

class _FolderManifestPreviewState extends State<_FolderManifestPreview> {
  bool _expanded = false;

  @override
  Widget build(BuildContext context) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        InkWell(
          onTap: () => setState(() => _expanded = !_expanded),
          child: Row(
            children: [
              Icon(
                _expanded ? Icons.expand_less : Icons.expand_more,
                size: 18,
                color: HavenTheme.textMuted,
              ),
              const SizedBox(width: 4),
              Text(
                _expanded ? 'Hide file list' : 'Show file list',
                style: TextStyle(fontSize: 12, color: HavenTheme.textMuted),
              ),
            ],
          ),
        ),
        if (_expanded) ...[
          const SizedBox(height: 8),
          Container(
            constraints: const BoxConstraints(maxHeight: 200),
            child: ListView.builder(
              shrinkWrap: true,
              itemCount: widget.manifest.length,
              itemBuilder: (context, index) {
                final entry = widget.manifest[index];
                return Padding(
                  padding: const EdgeInsets.symmetric(vertical: 1),
                  child: Row(
                    children: [
                      const SizedBox(width: 8),
                      Icon(Icons.insert_drive_file, size: 14, color: HavenTheme.textMuted),
                      const SizedBox(width: 6),
                      Expanded(
                        child: Text(
                          entry.relativePath,
                          style: TextStyle(fontSize: 12, color: HavenTheme.textSecondary),
                          overflow: TextOverflow.ellipsis,
                        ),
                      ),
                      Text(
                        _formatBytes(entry.size),
                        style: TextStyle(fontSize: 11, color: HavenTheme.textMuted),
                      ),
                    ],
                  ),
                );
              },
            ),
          ),
        ],
      ],
    );
  }
}

/// Active folder transfer card — the centerpiece visual.
class _ActiveFolderCard extends ConsumerStatefulWidget {
  final FolderTransfer folder;
  final Map<String, FileTransfer> allTransfers;
  const _ActiveFolderCard({required this.folder, required this.allTransfers});

  @override
  ConsumerState<_ActiveFolderCard> createState() => _ActiveFolderCardState();
}

class _ActiveFolderCardState extends ConsumerState<_ActiveFolderCard> {
  bool _expanded = false;

  @override
  Widget build(BuildContext context) {
    final folder = widget.folder;
    final allTransfers = widget.allTransfers;
    final done = folder.filesComplete(allTransfers);
    final progress = folder.progress(allTransfers);
    final bytesDone = folder.bytesDone(allTransfers);
    final active = folder.activeFile(allTransfers);

    // Determine the dominant phase color
    Color barColor;
    String phaseText;
    if (active == null && done == folder.fileCount) {
      barColor = _colorComplete;
      phaseText = 'Complete';
    } else if (active != null && active.state == TransferState.hashing) {
      barColor = _colorHashing;
      phaseText = 'Hashing...';
    } else if (active != null && active.state == TransferState.transferring) {
      barColor = folder.isUpload ? _colorUploading : _colorDownloading;
      phaseText = folder.isUpload ? 'Uploading...' : 'Downloading...';
    } else {
      barColor = _colorQueued;
      phaseText = 'Queued';
    }

    return Card(
      margin: const EdgeInsets.symmetric(horizontal: 16, vertical: 4),
      color: HavenTheme.surface,
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            // Header: folder icon + name + file counter
            Row(
              children: [
                Icon(Icons.folder, color: barColor, size: 24),
                const SizedBox(width: 10),
                Expanded(
                  child: Text(
                    folder.folderName,
                    style: const TextStyle(
                      color: HavenTheme.textPrimary,
                      fontWeight: FontWeight.bold,
                      fontSize: 15,
                    ),
                    overflow: TextOverflow.ellipsis,
                  ),
                ),
                Text(
                  '$done/${folder.fileCount} files',
                  style: TextStyle(fontSize: 12, color: HavenTheme.textMuted),
                ),
                const SizedBox(width: 4),
                IconButton(
                  icon: const Icon(Icons.close, size: 18, color: HavenTheme.error),
                  onPressed: () {
                    ref.read(fileTransferProvider.notifier).cancelAndRemoveFolder(folder.folderId);
                  },
                  tooltip: 'Cancel & Remove',
                  constraints: const BoxConstraints(minWidth: 32, minHeight: 32),
                ),
              ],
            ),

            const SizedBox(height: 12),

            // Main progress bar
            ClipRRect(
              borderRadius: BorderRadius.circular(4),
              child: LinearProgressIndicator(
                value: progress.clamp(0.0, 1.0),
                backgroundColor: HavenTheme.divider,
                valueColor: AlwaysStoppedAnimation(barColor),
                minHeight: 8,
              ),
            ),

            const SizedBox(height: 6),

            // Phase text + bytes counter
            Row(
              mainAxisAlignment: MainAxisAlignment.spaceBetween,
              children: [
                Row(
                  children: [
                    if (active != null) ...[
                      Icon(Icons.bolt, size: 14, color: barColor),
                      const SizedBox(width: 4),
                    ],
                    Text(
                      active != null
                          ? '$phaseText ${active.filename.split('/').last}'
                          : phaseText,
                      style: TextStyle(fontSize: 12, color: HavenTheme.textSecondary),
                      overflow: TextOverflow.ellipsis,
                    ),
                  ],
                ),
                Text(
                  '${_formatBytes(bytesDone)} / ${_formatBytes(folder.totalSize)}',
                  style: TextStyle(fontSize: 12, color: HavenTheme.textMuted),
                ),
              ],
            ),

            // Expandable file tree
            const SizedBox(height: 8),
            InkWell(
              onTap: () => setState(() => _expanded = !_expanded),
              child: Row(
                children: [
                  Icon(
                    _expanded ? Icons.expand_less : Icons.expand_more,
                    size: 18,
                    color: HavenTheme.textMuted,
                  ),
                  const SizedBox(width: 4),
                  Text(
                    'File details',
                    style: TextStyle(fontSize: 12, color: HavenTheme.textMuted),
                  ),
                ],
              ),
            ),

            if (_expanded) ...[
              const SizedBox(height: 8),
              _FolderFileTree(
                folder: folder,
                allTransfers: allTransfers,
              ),
            ],
          ],
        ),
      ),
    );
  }
}

/// Renders the per-file tree inside an active folder card.
class _FolderFileTree extends StatelessWidget {
  final FolderTransfer folder;
  final Map<String, FileTransfer> allTransfers;
  const _FolderFileTree({required this.folder, required this.allTransfers});

  @override
  Widget build(BuildContext context) {
    // Build a list of (relativePath, FileTransfer?) pairs
    // Use transferIdToPath for matched files, fall back to manifest for unmatched
    final entries = <_FileTreeEntry>[];

    // Collect all matched transfers
    final matchedPaths = <String>{};
    for (final entry in folder.transferIdToPath.entries) {
      final transfer = allTransfers[entry.key];
      matchedPaths.add(entry.value);
      entries.add(_FileTreeEntry(
        relativePath: entry.value,
        transfer: transfer,
        size: transfer?.fileSize ?? 0,
      ));
    }

    // Add manifest entries not yet matched (files not yet offered)
    for (final m in folder.manifest) {
      if (!matchedPaths.contains(m.relativePath)) {
        entries.add(_FileTreeEntry(
          relativePath: m.relativePath,
          transfer: null,
          size: m.size,
        ));
      }
    }

    // Sort by path
    entries.sort((a, b) => a.relativePath.compareTo(b.relativePath));

    // Group by directory
    final grouped = <String, List<_FileTreeEntry>>{};
    for (final e in entries) {
      final parts = e.relativePath.split('/');
      final dir = parts.length > 1 ? parts.sublist(0, parts.length - 1).join('/') : '';
      grouped.putIfAbsent(dir, () => []).add(e);
    }

    return Container(
      constraints: const BoxConstraints(maxHeight: 300),
      child: ListView(
        shrinkWrap: true,
        children: [
          for (final dirEntry in grouped.entries) ...[
            if (dirEntry.key.isNotEmpty)
              Padding(
                padding: const EdgeInsets.only(top: 4, bottom: 2),
                child: Row(
                  children: [
                    Icon(Icons.folder_outlined, size: 14, color: HavenTheme.textMuted),
                    const SizedBox(width: 4),
                    Text(
                      '${dirEntry.key}/',
                      style: TextStyle(fontSize: 12, color: HavenTheme.textMuted, fontWeight: FontWeight.w500),
                    ),
                  ],
                ),
              ),
            ...dirEntry.value.map((e) => _buildFileRow(e, dirEntry.key.isNotEmpty)),
          ],
        ],
      ),
    );
  }

  Widget _buildFileRow(_FileTreeEntry entry, bool indented) {
    final filename = entry.relativePath.split('/').last;
    final t = entry.transfer;

    // Status icon + color
    IconData icon;
    Color iconColor;
    if (t == null || t.state == TransferState.idle) {
      icon = Icons.circle_outlined;
      iconColor = _colorQueued;
    } else if (t.state == TransferState.hashing) {
      icon = Icons.hourglass_top;
      iconColor = _colorHashing;
    } else if (t.state == TransferState.transferring) {
      icon = Icons.sync;
      iconColor = t.isUpload ? _colorUploading : _colorDownloading;
    } else if (t.state == TransferState.complete) {
      icon = Icons.check_circle;
      iconColor = _colorComplete;
    } else {
      icon = Icons.error;
      iconColor = _colorError;
    }

    final isActive = t != null &&
        (t.state == TransferState.hashing || t.state == TransferState.transferring);

    return Container(
      color: isActive ? HavenTheme.surfaceVariant.withValues(alpha: 0.3) : null,
      padding: EdgeInsets.only(
        left: indented ? 20.0 : 8.0,
        top: 2,
        bottom: 2,
        right: 4,
      ),
      child: Row(
        children: [
          Icon(icon, size: 14, color: iconColor),
          const SizedBox(width: 6),
          Expanded(
            child: Text(
              filename,
              style: TextStyle(
                fontSize: 12,
                color: isActive ? HavenTheme.textPrimary : HavenTheme.textSecondary,
              ),
              overflow: TextOverflow.ellipsis,
            ),
          ),
          // Mini progress for active files
          if (isActive && t.state == TransferState.transferring) ...[
            SizedBox(
              width: 60,
              child: ClipRRect(
                borderRadius: BorderRadius.circular(2),
                child: LinearProgressIndicator(
                  value: t.progress.clamp(0.0, 1.0),
                  backgroundColor: HavenTheme.divider,
                  valueColor: AlwaysStoppedAnimation(iconColor),
                  minHeight: 4,
                ),
              ),
            ),
            const SizedBox(width: 6),
            Text(
              '${(t.progress * 100).toInt()}%',
              style: TextStyle(fontSize: 11, color: iconColor),
            ),
            const SizedBox(width: 8),
          ],
          Text(
            _formatBytes(entry.size),
            style: TextStyle(fontSize: 11, color: HavenTheme.textMuted),
          ),
        ],
      ),
    );
  }
}

class _FileTreeEntry {
  final String relativePath;
  final FileTransfer? transfer;
  final int size;
  _FileTreeEntry({required this.relativePath, this.transfer, required this.size});
}

/// Completed folder card.
class _CompletedFolderCard extends ConsumerWidget {
  final FolderTransfer folder;
  final Map<String, FileTransfer> allTransfers;
  const _CompletedFolderCard({required this.folder, required this.allTransfers});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    return Card(
      margin: const EdgeInsets.symmetric(horizontal: 16, vertical: 4),
      color: HavenTheme.surface,
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(8),
        side: const BorderSide(color: _colorComplete, width: 1),
      ),
      child: Padding(
        padding: const EdgeInsets.all(12),
        child: Row(
          children: [
            const Icon(Icons.folder, color: _colorComplete, size: 24),
            const SizedBox(width: 10),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    folder.folderName,
                    style: const TextStyle(
                      color: HavenTheme.textPrimary,
                      fontWeight: FontWeight.w500,
                    ),
                  ),
                  Text(
                    '${folder.isUpload ? "Sent" : "Received"} — ${folder.fileCount} files, ${_formatBytes(folder.totalSize)}',
                    style: TextStyle(fontSize: 12, color: HavenTheme.textMuted),
                  ),
                ],
              ),
            ),
            IconButton(
              icon: const Icon(Icons.close, size: 16, color: HavenTheme.textMuted),
              onPressed: () {
                ref.read(fileTransferProvider.notifier).removeFolder(folder.folderId);
              },
              tooltip: 'Dismiss',
              constraints: const BoxConstraints(minWidth: 32, minHeight: 32),
            ),
          ],
        ),
      ),
    );
  }
}
