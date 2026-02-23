import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import 'package:haven_app/config/constants.dart';
import 'package:haven_app/models/channel.dart';
import 'package:haven_app/providers/auth_provider.dart';
import 'package:haven_app/providers/channel_provider.dart';
import 'package:haven_app/providers/gateway_provider.dart';
import 'package:haven_app/providers/message_provider.dart';
import 'package:haven_app/providers/presence_provider.dart';
import 'package:haven_app/providers/typing_provider.dart';
import 'package:haven_app/providers/file_transfer_provider.dart';
import 'package:haven_app/providers/voice_provider.dart';
import 'package:haven_app/screens/chat_screen.dart';
import 'package:haven_app/screens/file_transfer_screen.dart';
import 'package:haven_app/widgets/sidebar.dart';
import 'package:haven_app/widgets/voice_controls.dart';

class HomeScreen extends ConsumerStatefulWidget {
  const HomeScreen({super.key});

  @override
  ConsumerState<HomeScreen> createState() => _HomeScreenState();
}

class _HomeScreenState extends ConsumerState<HomeScreen> {
  bool _gatewayInitialized = false;

  @override
  void initState() {
    super.initState();
    // Defer gateway initialization until after first build
    WidgetsBinding.instance.addPostFrameCallback((_) {
      _initGateway();
    });
  }

  void _initGateway() {
    if (_gatewayInitialized) return;
    _gatewayInitialized = true;

    final gateway = ref.read(gatewayServiceProvider);
    final authState = ref.read(authProvider);

    // Register gateway event handlers
    gateway.on('Ready', (event) {
      // Subscribe to all channels after Ready
      final channels = ref.read(channelsProvider);
      gateway.subscribe(channels.map((c) => c.id).toList());

      // Load initial messages
      ref.read(messageProvider.notifier).loadMessages();

      // Initialize HTP file transfer service
      ref.read(fileTransferProvider.notifier).init();
    });

    gateway.on('MessageCreate', (event) {
      final data = event['data'] as Map<String, dynamic>;
      final channelId = data['channel_id'] as String;
      if (channelId == HavenConstants.generalChannelId) {
        ref.read(messageProvider.notifier).handleIncomingMessage(event);
      }
    });

    gateway.on('PresenceUpdate', (event) {
      final data = event['data'] as Map<String, dynamic>;
      final userId = data['user_id'] as String;
      final username = data['username'] as String;
      final online = data['online'] as bool;

      if (online) {
        ref.read(presenceProvider.notifier).userOnline(userId, username);
      } else {
        ref.read(presenceProvider.notifier).userOffline(userId);
      }
    });

    gateway.on('TypingStart', (event) {
      final data = event['data'] as Map<String, dynamic>;
      final userId = data['user_id'] as String;
      final username = data['username'] as String;

      // Don't show our own typing
      if (userId != authState.userId) {
        ref.read(typingProvider.notifier).userTyping(userId, username);
      }
    });

    gateway.on('ReactionAdd', (event) {
      ref.read(messageProvider.notifier).handleReactionAdd(event);
    });

    gateway.on('ReactionRemove', (event) {
      ref.read(messageProvider.notifier).handleReactionRemove(event);
    });

    gateway.on('VoiceStateUpdate', (event) {
      ref.read(voiceProvider.notifier).handleVoiceStateUpdate(event);
    });

    gateway.on('VoiceAudioData', (event) {
      ref.read(voiceProvider.notifier).handleAudioData(event);
    });

    // HTP file transfer event handlers
    for (final event in [
      'HtpOffer',
      'HtpAccept',
      'HtpNack',
      'HtpRtt',
      'HtpAck',
      'HtpDone',
      'HtpCancel',
    ]) {
      gateway.on(event, (data) {
        ref.read(fileTransferProvider.notifier).handleControlMessage(data);
      });
    }

    gateway.onDisconnect(() {
      // Clear presence on disconnect — will repopulate on reconnect
      ref.read(presenceProvider.notifier).clear();
    });

    // Connect
    gateway.connect();
  }

  @override
  void dispose() {
    if (_gatewayInitialized) {
      ref.read(gatewayServiceProvider).disconnect();
    }
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final activeChannel = ref.watch(activeChannelProvider);

    return Scaffold(
      body: Row(
        children: [
          // Sidebar
          const Sidebar(),

          // Divider
          const VerticalDivider(width: 1, thickness: 1),

          // Main content area
          Expanded(
            child: _buildContent(activeChannel),
          ),
        ],
      ),
    );
  }

  Widget _buildContent(Channel channel) {
    switch (channel.type) {
      case ChannelType.text:
        return const ChatScreen();
      case ChannelType.voice:
        return const VoiceControls();
      case ChannelType.file:
        return const FileTransferScreen();
    }
  }
}
