import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import 'package:haven_app/config/theme.dart';
import 'package:haven_app/providers/auth_provider.dart';

class SettingsDialog extends ConsumerWidget {
  const SettingsDialog({super.key});

  static Future<void> show(BuildContext context) {
    return showDialog(
      context: context,
      builder: (_) => const SettingsDialog(),
    );
  }

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final authState = ref.watch(authProvider);
    final serverUrl = ref.read(authProvider.notifier).serverUrl;

    return Dialog(
      backgroundColor: HavenTheme.surface,
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(16)),
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 420, maxHeight: 480),
        child: Padding(
          padding: const EdgeInsets.all(24),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              // Header
              Row(
                children: [
                  const Icon(Icons.settings, color: HavenTheme.textPrimary),
                  const SizedBox(width: 8),
                  const Text(
                    'Settings',
                    style: TextStyle(
                      fontSize: 18,
                      fontWeight: FontWeight.w600,
                      color: HavenTheme.textPrimary,
                    ),
                  ),
                  const Spacer(),
                  IconButton(
                    icon: const Icon(Icons.close, size: 20),
                    onPressed: () => Navigator.of(context).pop(),
                    color: HavenTheme.textMuted,
                  ),
                ],
              ),
              const SizedBox(height: 20),

              // Account section
              _SectionHeader(title: 'ACCOUNT'),
              const SizedBox(height: 8),
              _InfoRow(label: 'Username', value: authState.username ?? '—'),
              const SizedBox(height: 4),
              _InfoRow(
                label: 'User ID',
                value: authState.userId ?? '—',
                mono: true,
              ),
              const SizedBox(height: 16),

              // Connection section
              _SectionHeader(title: 'CONNECTION'),
              const SizedBox(height: 8),
              _InfoRow(label: 'Server', value: serverUrl),
              const SizedBox(height: 4),
              _InfoRow(
                label: 'Encryption',
                value: 'AES-256-GCM',
              ),
              const SizedBox(height: 4),
              _InfoRow(
                label: 'Status',
                value: authState.status == AuthStatus.authenticated
                    ? 'Connected'
                    : 'Disconnected',
                valueColor: authState.status == AuthStatus.authenticated
                    ? HavenTheme.online
                    : HavenTheme.error,
              ),

              const SizedBox(height: 24),

              // About section
              _SectionHeader(title: 'ABOUT'),
              const SizedBox(height: 8),
              _InfoRow(label: 'Version', value: '2.0.0'),
              const SizedBox(height: 4),
              _InfoRow(label: 'Platform', value: 'Flutter Desktop'),

              const Spacer(),

              // Logout button
              SizedBox(
                width: double.infinity,
                child: OutlinedButton.icon(
                  onPressed: () {
                    Navigator.of(context).pop();
                    ref.read(authProvider.notifier).logout();
                  },
                  icon: const Icon(Icons.logout, size: 18),
                  label: const Text('Sign Out'),
                  style: OutlinedButton.styleFrom(
                    foregroundColor: HavenTheme.error,
                    side: const BorderSide(color: HavenTheme.error),
                    padding: const EdgeInsets.symmetric(vertical: 12),
                    shape: RoundedRectangleBorder(
                      borderRadius: BorderRadius.circular(8),
                    ),
                  ),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

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

class _InfoRow extends StatelessWidget {
  final String label;
  final String value;
  final bool mono;
  final Color? valueColor;

  const _InfoRow({
    required this.label,
    required this.value,
    this.mono = false,
    this.valueColor,
  });

  @override
  Widget build(BuildContext context) {
    return Row(
      children: [
        SizedBox(
          width: 100,
          child: Text(
            label,
            style: TextStyle(
              fontSize: 13,
              color: HavenTheme.textSecondary,
            ),
          ),
        ),
        Expanded(
          child: Text(
            value,
            style: TextStyle(
              fontSize: 13,
              color: valueColor ?? HavenTheme.textPrimary,
              fontFamily: mono ? 'monospace' : null,
            ),
            overflow: TextOverflow.ellipsis,
          ),
        ),
      ],
    );
  }
}
