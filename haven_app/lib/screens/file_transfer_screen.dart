import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:file_picker/file_picker.dart';
import 'package:path_provider/path_provider.dart';

import 'package:haven_app/config/constants.dart';
import 'package:haven_app/config/theme.dart';
import 'package:haven_app/providers/auth_provider.dart';
import 'package:haven_app/providers/file_transfer_provider.dart';
import 'package:haven_app/providers/presence_provider.dart';
import 'package:haven_app/services/file_client_bindings.dart';
import 'package:haven_app/services/file_transfer_service.dart';

class FileTransferScreen extends ConsumerWidget {
  const FileTransferScreen({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final transferState = ref.watch(fileTransferProvider);
    final onlineUsers = ref.watch(presenceProvider);

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

        // Pending offers
        if (transferState.pendingOffers.isNotEmpty) ...[
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 16, 16, 8),
            child: Row(
              children: [
                Text(
                  'INCOMING OFFERS',
                  style: TextStyle(
                    fontSize: 11,
                    fontWeight: FontWeight.w600,
                    color: HavenTheme.textMuted,
                    letterSpacing: 1.2,
                  ),
                ),
              ],
            ),
          ),
          ...transferState.pendingOffers.map((t) => _PendingOfferTile(transfer: t)),
        ],

        // Active transfers
        if (transferState.active.isNotEmpty) ...[
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 16, 16, 8),
            child: Row(
              children: [
                Text(
                  'ACTIVE TRANSFERS',
                  style: TextStyle(
                    fontSize: 11,
                    fontWeight: FontWeight.w600,
                    color: HavenTheme.textMuted,
                    letterSpacing: 1.2,
                  ),
                ),
              ],
            ),
          ),
          ...transferState.active.map((t) => _ActiveTransferTile(transfer: t)),
        ],

        // Completed transfers
        if (transferState.completed.isNotEmpty) ...[
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 16, 16, 8),
            child: Row(
              children: [
                Text(
                  'COMPLETED',
                  style: TextStyle(
                    fontSize: 11,
                    fontWeight: FontWeight.w600,
                    color: HavenTheme.textMuted,
                    letterSpacing: 1.2,
                  ),
                ),
              ],
            ),
          ),
          ...transferState.completed.map((t) => _CompletedTransferTile(transfer: t)),
        ],

        // Empty state
        if (transferState.transfers.isEmpty)
          Expanded(
            child: Center(
              child: Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Icon(Icons.folder_open, size: 64, color: HavenTheme.textMuted),
                  const SizedBox(height: 16),
                  Text(
                    'No file transfers',
                    style: TextStyle(
                      fontSize: 16,
                      color: HavenTheme.textMuted,
                    ),
                  ),
                  const SizedBox(height: 8),
                  Text(
                    'Click "Send File" to transfer a file to another user.',
                    style: TextStyle(
                      fontSize: 13,
                      color: HavenTheme.textMuted,
                    ),
                  ),
                ],
              ),
            ),
          ),
      ],
    );
  }

  void _showSendDialog(BuildContext context, WidgetRef ref, Map<String, dynamic> onlineUsers) async {
    // Pick a file first
    final result = await FilePicker.platform.pickFiles();
    if (result == null || result.files.isEmpty) return;

    final file = result.files.first;
    if (file.path == null) return;

    // Show user picker dialog
    if (!context.mounted) return;
    final targetUserId = await showDialog<String>(
      context: context,
      builder: (ctx) => _UserPickerDialog(onlineUsers: onlineUsers),
    );

    if (targetUserId == null) return;

    // Use default channel key for MVP
    ref.read(fileTransferProvider.notifier).sendFile(
      filePath: file.path!,
      filename: file.name,
      fileSize: file.size,
      targetUserId: targetUserId,
      masterKey: HavenConstants.defaultChannelKey,
      salt: 'file-transfer',
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
                    style: TextStyle(
                      fontSize: 12,
                      color: HavenTheme.textMuted,
                    ),
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
                  color: HavenTheme.primaryLight,
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
                  icon: const Icon(Icons.close, size: 18),
                  onPressed: () {
                    ref.read(fileTransferProvider.notifier).cancelTransfer(transfer.transferId);
                  },
                  tooltip: 'Cancel',
                  constraints: const BoxConstraints(minWidth: 32, minHeight: 32),
                ),
              ],
            ),
            const SizedBox(height: 8),
            LinearProgressIndicator(
              value: transfer.progress,
              backgroundColor: HavenTheme.divider,
              valueColor: const AlwaysStoppedAnimation(HavenTheme.primaryLight),
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
