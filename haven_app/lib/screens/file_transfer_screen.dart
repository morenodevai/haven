import 'package:file_picker/file_picker.dart';
import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import 'package:haven_app/config/theme.dart';
import 'package:haven_app/models/user.dart';
import 'package:haven_app/providers/auth_provider.dart';
import 'package:haven_app/providers/file_transfer_provider.dart';
import 'package:haven_app/providers/presence_provider.dart';
import 'package:haven_app/services/htp_service.dart';

class FileTransferScreen extends ConsumerStatefulWidget {
  const FileTransferScreen({super.key});

  @override
  ConsumerState<FileTransferScreen> createState() => _FileTransferScreenState();
}

class _FileTransferScreenState extends ConsumerState<FileTransferScreen> {
  String? _selectedUserId;

  @override
  Widget build(BuildContext context) {
    final ftState = ref.watch(fileTransferProvider);
    final onlineUsers = ref.watch(presenceProvider);
    final authState = ref.watch(authProvider);

    // Filter out self from online users
    final recipients = onlineUsers.values
        .where((u) => u.id != authState.userId)
        .toList();

    // Listen for errors
    ref.listen<FileTransferState>(fileTransferProvider, (prev, next) {
      if (next.error != null && next.error != prev?.error) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text(next.error!)),
        );
      }
    });

    // Separate transfers by state
    final activeTransfers = ftState.transfers.values
        .where((t) =>
            t.state == TransferState.active ||
            t.state == TransferState.pending)
        .toList();
    final completedTransfers = ftState.transfers.values
        .where((t) =>
            t.state == TransferState.complete ||
            t.state == TransferState.failed ||
            t.state == TransferState.cancelled)
        .toList();

    return Column(
      children: [
        // Header
        Container(
          padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
          decoration: const BoxDecoration(
            border: Border(bottom: BorderSide(color: HavenTheme.divider)),
          ),
          child: Row(
            children: [
              const Icon(Icons.tag, size: 20, color: HavenTheme.textMuted),
              const SizedBox(width: 8),
              const Text(
                'file-sharing',
                style: TextStyle(
                  fontSize: 16,
                  fontWeight: FontWeight.w600,
                  color: HavenTheme.textPrimary,
                ),
              ),
              const Spacer(),
              Icon(Icons.lock_outline, size: 16, color: HavenTheme.textMuted),
              const SizedBox(width: 4),
              Text(
                'End-to-end encrypted',
                style: TextStyle(fontSize: 12, color: HavenTheme.textMuted),
              ),
            ],
          ),
        ),

        // Content
        Expanded(
          child: ListView(
            padding: const EdgeInsets.all(16),
            children: [
              // Send File section
              _SectionHeader(title: 'SEND FILE'),
              const SizedBox(height: 8),
              Container(
                padding: const EdgeInsets.all(16),
                decoration: BoxDecoration(
                  color: HavenTheme.surfaceVariant.withValues(alpha: 0.3),
                  borderRadius: BorderRadius.circular(8),
                  border: Border.all(color: HavenTheme.divider),
                ),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    // Recipient dropdown
                    Row(
                      children: [
                        Text(
                          'To:',
                          style: TextStyle(
                            fontSize: 14,
                            color: HavenTheme.textSecondary,
                          ),
                        ),
                        const SizedBox(width: 12),
                        Expanded(
                          child: _RecipientDropdown(
                            recipients: recipients,
                            selectedUserId: _selectedUserId,
                            onChanged: (userId) {
                              setState(() => _selectedUserId = userId);
                            },
                          ),
                        ),
                      ],
                    ),
                    const SizedBox(height: 12),
                    // Pick file button
                    ElevatedButton.icon(
                      onPressed: _selectedUserId == null
                          ? null
                          : () => _pickAndSendFile(),
                      icon: const Icon(Icons.attach_file, size: 18),
                      label: const Text('Pick File'),
                      style: ElevatedButton.styleFrom(
                        backgroundColor: HavenTheme.primaryLight,
                        foregroundColor: Colors.white,
                        disabledBackgroundColor:
                            HavenTheme.surfaceVariant.withValues(alpha: 0.5),
                      ),
                    ),
                  ],
                ),
              ),

              const SizedBox(height: 24),

              // Active Transfers
              if (activeTransfers.isNotEmpty) ...[
                _SectionHeader(title: 'ACTIVE TRANSFERS'),
                const SizedBox(height: 8),
                ...activeTransfers.map((t) => _TransferCard(transfer: t)),
                const SizedBox(height: 24),
              ],

              // Completed Transfers
              if (completedTransfers.isNotEmpty) ...[
                _SectionHeader(title: 'COMPLETED'),
                const SizedBox(height: 8),
                ...completedTransfers.map((t) => _CompletedTransferTile(transfer: t)),
              ],
            ],
          ),
        ),
      ],
    );
  }

  Future<void> _pickAndSendFile() async {
    final result = await FilePicker.platform.pickFiles();
    if (result == null || result.files.isEmpty) return;

    final filePath = result.files.single.path;
    if (filePath == null) return;

    ref
        .read(fileTransferProvider.notifier)
        .sendFile(filePath, _selectedUserId!);
  }

  void _showIncomingOfferDialog(
      BuildContext context, Map<String, dynamic> offer) {
    final filename = offer['filename'] as String? ?? 'unknown';
    final size = offer['size'] as int? ?? 0;
    final fromUser = offer['from_user_id'] as String? ?? 'unknown';

    final sizeStr = _formatSize(size);

    showDialog(
      context: context,
      barrierDismissible: false,
      builder: (ctx) => AlertDialog(
        backgroundColor: HavenTheme.surface,
        title: const Text('Incoming File Transfer',
            style: TextStyle(color: HavenTheme.textPrimary)),
        content: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text('From: $fromUser',
                style: TextStyle(color: HavenTheme.textSecondary)),
            const SizedBox(height: 4),
            Text('File: $filename',
                style: TextStyle(color: HavenTheme.textSecondary)),
            const SizedBox(height: 4),
            Text('Size: $sizeStr',
                style: TextStyle(color: HavenTheme.textSecondary)),
          ],
        ),
        actions: [
          TextButton(
            onPressed: () {
              ref.read(fileTransferProvider.notifier).rejectOffer();
              Navigator.of(ctx).pop();
            },
            child:
                Text('Reject', style: TextStyle(color: HavenTheme.error)),
          ),
          ElevatedButton(
            onPressed: () async {
              Navigator.of(ctx).pop();
              final savePath = await FilePicker.platform.saveFile(
                dialogTitle: 'Save file as...',
                fileName: filename,
              );
              if (savePath != null) {
                ref
                    .read(fileTransferProvider.notifier)
                    .acceptOffer(savePath);
              } else {
                ref.read(fileTransferProvider.notifier).rejectOffer();
              }
            },
            style: ElevatedButton.styleFrom(
              backgroundColor: HavenTheme.primaryLight,
              foregroundColor: Colors.white,
            ),
            child: const Text('Accept'),
          ),
        ],
      ),
    );
  }

  String _formatSize(int bytes) {
    if (bytes > 1000000000) {
      return '${(bytes / 1000000000).toStringAsFixed(2)} GB';
    } else if (bytes > 1000000) {
      return '${(bytes / 1000000).toStringAsFixed(1)} MB';
    } else if (bytes > 1000) {
      return '${(bytes / 1000).toStringAsFixed(1)} KB';
    }
    return '$bytes B';
  }
}

// ── Widgets ──

class _SectionHeader extends StatelessWidget {
  final String title;
  const _SectionHeader({required this.title});

  @override
  Widget build(BuildContext context) {
    return Text(
      title,
      style: TextStyle(
        fontSize: 11,
        fontWeight: FontWeight.w600,
        color: HavenTheme.textMuted,
        letterSpacing: 1.2,
      ),
    );
  }
}

class _RecipientDropdown extends StatelessWidget {
  final List<User> recipients;
  final String? selectedUserId;
  final ValueChanged<String?> onChanged;

  const _RecipientDropdown({
    required this.recipients,
    required this.selectedUserId,
    required this.onChanged,
  });

  @override
  Widget build(BuildContext context) {
    if (recipients.isEmpty) {
      return Text(
        'No users online',
        style: TextStyle(fontSize: 14, color: HavenTheme.textMuted),
      );
    }

    return DropdownButton<String>(
      value: selectedUserId,
      hint: Text('Select recipient',
          style: TextStyle(color: HavenTheme.textMuted)),
      isExpanded: true,
      dropdownColor: HavenTheme.surface,
      style: const TextStyle(color: HavenTheme.textPrimary),
      underline: Container(height: 1, color: HavenTheme.divider),
      items: recipients.map((user) {
        return DropdownMenuItem<String>(
          value: user.id,
          child: Text(user.username),
        );
      }).toList(),
      onChanged: onChanged,
    );
  }
}

class _TransferCard extends ConsumerWidget {
  final HtpTransfer transfer;
  const _TransferCard({required this.transfer});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final directionIcon = transfer.direction == TransferDirection.sending
        ? Icons.arrow_upward
        : Icons.arrow_downward;
    final directionLabel =
        transfer.direction == TransferDirection.sending ? 'Sending' : 'Receiving';

    final bytesTransferred =
        (transfer.progress * transfer.fileSize).toInt();

    return Container(
      margin: const EdgeInsets.only(bottom: 8),
      padding: const EdgeInsets.all(12),
      decoration: BoxDecoration(
        color: HavenTheme.surfaceVariant.withValues(alpha: 0.3),
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: HavenTheme.divider),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          // File info row
          Row(
            children: [
              Icon(directionIcon, size: 16, color: HavenTheme.primaryLight),
              const SizedBox(width: 8),
              Expanded(
                child: Text(
                  '$directionLabel: ${transfer.filename}',
                  style: const TextStyle(
                    fontSize: 14,
                    fontWeight: FontWeight.w500,
                    color: HavenTheme.textPrimary,
                  ),
                  overflow: TextOverflow.ellipsis,
                ),
              ),
            ],
          ),
          const SizedBox(height: 8),

          // Progress bar
          ClipRRect(
            borderRadius: BorderRadius.circular(4),
            child: LinearProgressIndicator(
              value: transfer.progress,
              backgroundColor: HavenTheme.surfaceVariant,
              valueColor:
                  AlwaysStoppedAnimation<Color>(HavenTheme.primaryLight),
              minHeight: 6,
            ),
          ),
          const SizedBox(height: 6),

          // Stats row
          Row(
            children: [
              Text(
                '${(transfer.progress * 100).toStringAsFixed(0)}%',
                style: TextStyle(
                    fontSize: 12, color: HavenTheme.textSecondary),
              ),
              const SizedBox(width: 12),
              Text(
                transfer.rateFormatted,
                style: TextStyle(
                    fontSize: 12, color: HavenTheme.textSecondary),
              ),
              const Spacer(),
              Text(
                '${_formatSize(bytesTransferred)} / ${transfer.sizeFormatted}',
                style: TextStyle(
                    fontSize: 12, color: HavenTheme.textMuted),
              ),
              if (transfer.retransmits > 0) ...[
                const SizedBox(width: 8),
                Text(
                  '${transfer.retransmits} retransmits',
                  style: TextStyle(
                      fontSize: 11, color: HavenTheme.textMuted),
                ),
              ],
              const SizedBox(width: 8),
              InkWell(
                onTap: () {
                  ref
                      .read(fileTransferProvider.notifier)
                      .cancelTransfer(transfer.sessionId);
                },
                child: Text(
                  'Cancel',
                  style: TextStyle(
                    fontSize: 12,
                    color: HavenTheme.error,
                    fontWeight: FontWeight.w500,
                  ),
                ),
              ),
            ],
          ),
        ],
      ),
    );
  }

  String _formatSize(int bytes) {
    if (bytes > 1000000000) {
      return '${(bytes / 1000000000).toStringAsFixed(2)} GB';
    } else if (bytes > 1000000) {
      return '${(bytes / 1000000).toStringAsFixed(1)} MB';
    } else if (bytes > 1000) {
      return '${(bytes / 1000).toStringAsFixed(1)} KB';
    }
    return '$bytes B';
  }
}

class _CompletedTransferTile extends StatelessWidget {
  final HtpTransfer transfer;
  const _CompletedTransferTile({required this.transfer});

  @override
  Widget build(BuildContext context) {
    final IconData icon;
    final Color iconColor;

    switch (transfer.state) {
      case TransferState.complete:
        icon = Icons.check_circle_outline;
        iconColor = HavenTheme.online;
        break;
      case TransferState.failed:
        icon = Icons.error_outline;
        iconColor = HavenTheme.error;
        break;
      case TransferState.cancelled:
        icon = Icons.cancel_outlined;
        iconColor = HavenTheme.textMuted;
        break;
      default:
        icon = Icons.help_outline;
        iconColor = HavenTheme.textMuted;
    }

    final elapsed = transfer.elapsed;
    final elapsedStr = elapsed.inSeconds < 60
        ? '${elapsed.inSeconds}.${(elapsed.inMilliseconds % 1000 ~/ 100)}s'
        : '${elapsed.inMinutes}m ${elapsed.inSeconds % 60}s';

    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 4),
      child: Row(
        children: [
          Icon(icon, size: 16, color: iconColor),
          const SizedBox(width: 8),
          Expanded(
            child: Text(
              transfer.filename,
              style: TextStyle(fontSize: 13, color: HavenTheme.textSecondary),
              overflow: TextOverflow.ellipsis,
            ),
          ),
          Text(
            transfer.sizeFormatted,
            style: TextStyle(fontSize: 12, color: HavenTheme.textMuted),
          ),
          const SizedBox(width: 12),
          Text(
            elapsedStr,
            style: TextStyle(fontSize: 12, color: HavenTheme.textMuted),
          ),
          if (transfer.state == TransferState.complete) ...[
            const SizedBox(width: 12),
            Text(
              transfer.rateFormatted,
              style: TextStyle(fontSize: 12, color: HavenTheme.textMuted),
            ),
          ],
        ],
      ),
    );
  }
}
