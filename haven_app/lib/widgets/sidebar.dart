import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import 'package:haven_app/config/theme.dart';
import 'package:haven_app/models/channel.dart';
import 'package:haven_app/models/user.dart';
import 'package:haven_app/providers/auth_provider.dart';
import 'package:haven_app/providers/channel_provider.dart';
import 'package:haven_app/providers/presence_provider.dart';
import 'package:haven_app/providers/voice_provider.dart';
import 'package:haven_app/widgets/settings_dialog.dart';

class Sidebar extends ConsumerWidget {
  const Sidebar({super.key});

  IconData _channelIcon(ChannelType type) {
    switch (type) {
      case ChannelType.text:
        return Icons.tag;
      case ChannelType.voice:
        return Icons.headset;
      case ChannelType.file:
        return Icons.folder_shared;
    }
  }

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final channels = ref.watch(channelsProvider);
    final activeChannel = ref.watch(activeChannelProvider);
    final onlineUsers = ref.watch(presenceProvider);
    final authState = ref.watch(authProvider);
    final voiceState = ref.watch(voiceProvider);

    return Container(
      width: 240,
      color: HavenTheme.sidebarBackground,
      child: Column(
        children: [
          // Header
          Container(
            padding: const EdgeInsets.all(16),
            decoration: const BoxDecoration(
              border: Border(
                bottom: BorderSide(color: HavenTheme.divider),
              ),
            ),
            child: Row(
              children: [
                Icon(Icons.shield_outlined,
                    color: HavenTheme.primaryLight, size: 24),
                const SizedBox(width: 8),
                const Text(
                  'Haven',
                  style: TextStyle(
                    fontSize: 18,
                    fontWeight: FontWeight.bold,
                    color: HavenTheme.textPrimary,
                  ),
                ),
                const Spacer(),
                Text(
                  'v2.0',
                  style: TextStyle(
                    fontSize: 12,
                    color: HavenTheme.textMuted,
                  ),
                ),
              ],
            ),
          ),

          // Channels
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 16, 16, 8),
            child: Row(
              children: [
                Text(
                  'CHANNELS',
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

          ...channels.map((channel) {
            final isActive = channel.id == activeChannel.id;
            return _ChannelTile(
              channel: channel,
              icon: _channelIcon(channel.type),
              isActive: isActive,
              onTap: () {
                ref.read(activeChannelProvider.notifier).state = channel;
              },
            );
          }),

          // Voice participants (if in voice)
          if (voiceState.isInVoice) ...[
            const SizedBox(height: 8),
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 16),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    'VOICE CONNECTED',
                    style: TextStyle(
                      fontSize: 11,
                      fontWeight: FontWeight.w600,
                      color: HavenTheme.online,
                      letterSpacing: 1.2,
                    ),
                  ),
                  const SizedBox(height: 4),
                  ...voiceState.participants.values.map((p) {
                    return Padding(
                      padding: const EdgeInsets.symmetric(vertical: 2),
                      child: Row(
                        children: [
                          Icon(
                            p.speaking ? Icons.volume_up : Icons.volume_off,
                            size: 14,
                            color: p.speaking
                                ? HavenTheme.online
                                : HavenTheme.textMuted,
                          ),
                          const SizedBox(width: 6),
                          Text(
                            p.username,
                            style: TextStyle(
                              fontSize: 13,
                              color: HavenTheme.textSecondary,
                            ),
                          ),
                          if (p.selfMute) ...[
                            const SizedBox(width: 4),
                            Icon(Icons.mic_off, size: 12,
                                color: HavenTheme.error),
                          ],
                          if (p.selfDeaf) ...[
                            const SizedBox(width: 4),
                            Icon(Icons.headset_off, size: 12,
                                color: HavenTheme.error),
                          ],
                        ],
                      ),
                    );
                  }),
                ],
              ),
            ),
          ],

          const Spacer(),

          // Online users
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 8, 16, 8),
            child: Row(
              children: [
                Text(
                  'ONLINE — ${onlineUsers.length}',
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

          SizedBox(
            height: (onlineUsers.length.clamp(0, 5)) * 32.0,
            child: ListView(
              padding: EdgeInsets.zero,
              children: onlineUsers.values.map((user) {
                return _OnlineUserTile(user: user);
              }).toList(),
            ),
          ),

          // User info bar
          Container(
            padding: const EdgeInsets.all(12),
            decoration: const BoxDecoration(
              border: Border(
                top: BorderSide(color: HavenTheme.divider),
              ),
            ),
            child: Row(
              children: [
                Container(
                  width: 32,
                  height: 32,
                  decoration: BoxDecoration(
                    color: HavenTheme.primaryLight,
                    borderRadius: BorderRadius.circular(16),
                  ),
                  child: Center(
                    child: Text(
                      (authState.username ?? '?')[0].toUpperCase(),
                      style: const TextStyle(
                        color: Colors.white,
                        fontWeight: FontWeight.bold,
                      ),
                    ),
                  ),
                ),
                const SizedBox(width: 8),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        authState.username ?? 'Unknown',
                        style: const TextStyle(
                          fontSize: 13,
                          fontWeight: FontWeight.w500,
                          color: HavenTheme.textPrimary,
                        ),
                        overflow: TextOverflow.ellipsis,
                      ),
                      Text(
                        'Online',
                        style: TextStyle(
                          fontSize: 11,
                          color: HavenTheme.online,
                        ),
                      ),
                    ],
                  ),
                ),
                IconButton(
                  icon: const Icon(Icons.settings, size: 18),
                  onPressed: () => SettingsDialog.show(context),
                  tooltip: 'Settings',
                  iconSize: 18,
                  constraints:
                      const BoxConstraints(minWidth: 32, minHeight: 32),
                ),
                IconButton(
                  icon: const Icon(Icons.logout, size: 18),
                  onPressed: () {
                    ref.read(authProvider.notifier).logout();
                  },
                  tooltip: 'Logout',
                  iconSize: 18,
                  constraints:
                      const BoxConstraints(minWidth: 32, minHeight: 32),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

class _ChannelTile extends StatelessWidget {
  final Channel channel;
  final IconData icon;
  final bool isActive;
  final VoidCallback onTap;

  const _ChannelTile({
    required this.channel,
    required this.icon,
    required this.isActive,
    required this.onTap,
  });

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 1),
      child: Material(
        color: isActive
            ? HavenTheme.surfaceVariant.withValues(alpha: 0.5)
            : Colors.transparent,
        borderRadius: BorderRadius.circular(6),
        child: InkWell(
          onTap: onTap,
          borderRadius: BorderRadius.circular(6),
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 8),
            child: Row(
              children: [
                Icon(
                  icon,
                  size: 18,
                  color: isActive
                      ? HavenTheme.textPrimary
                      : HavenTheme.textMuted,
                ),
                const SizedBox(width: 8),
                Text(
                  channel.name,
                  style: TextStyle(
                    fontSize: 14,
                    color: isActive
                        ? HavenTheme.textPrimary
                        : HavenTheme.textSecondary,
                    fontWeight:
                        isActive ? FontWeight.w500 : FontWeight.normal,
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class _OnlineUserTile extends StatelessWidget {
  final User user;

  const _OnlineUserTile({required this.user});

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 2),
      child: Row(
        children: [
          Container(
            width: 8,
            height: 8,
            decoration: BoxDecoration(
              color: HavenTheme.online,
              borderRadius: BorderRadius.circular(4),
            ),
          ),
          const SizedBox(width: 8),
          Text(
            user.username,
            style: const TextStyle(
              fontSize: 13,
              color: HavenTheme.textSecondary,
            ),
          ),
        ],
      ),
    );
  }
}
