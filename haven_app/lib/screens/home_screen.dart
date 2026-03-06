import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import 'package:haven_app/config/constants.dart';
import 'package:haven_app/models/channel.dart';
import 'package:haven_app/providers/auth_provider.dart';
import 'package:haven_app/providers/channel_provider.dart';
import 'package:haven_app/providers/gateway_provider.dart';
import 'package:haven_app/providers/message_provider.dart';
import 'package:haven_app/providers/file_transfer_provider.dart';
import 'package:haven_app/providers/presence_provider.dart';
import 'package:haven_app/providers/typing_provider.dart';
import 'package:haven_app/providers/video_provider.dart';
import 'package:haven_app/providers/voice_provider.dart';
import 'package:haven_app/services/file_transfer_service.dart';
import 'package:haven_app/screens/chat_screen.dart';
import 'package:haven_app/screens/file_transfer_screen.dart';
import 'package:haven_app/widgets/sidebar.dart';
import 'package:haven_app/widgets/video_grid.dart';
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
    WidgetsBinding.instance.addPostFrameCallback((_) {
      _initGateway();
    });
  }

  void _initGateway() {
    if (_gatewayInitialized) return;
    _gatewayInitialized = true;

    final gateway = ref.read(gatewayServiceProvider);
    final authState = ref.read(authProvider);

    gateway.on('Ready', (event) {
      final channels = ref.read(channelsProvider);
      gateway.subscribe(channels.map((c) => c.id).toList());
      ref.read(messageProvider.notifier).loadMessages();
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

      final vsData = event['data'] as Map<String, dynamic>;
      final vsUserId = vsData['user_id'] as String;
      final vsSessionId = vsData['session_id'] as String?;
      final myUserId = ref.read(authProvider).userId;
      if (vsUserId != myUserId) {
        if (vsSessionId != null) {
          ref.read(videoProvider.notifier).handlePeerJoined(vsUserId);
        } else {
          ref.read(videoProvider.notifier).handlePeerLeft(vsUserId);
        }
      }
    });

    gateway.on('VoiceAudioData', (event) {
      ref.read(voiceProvider.notifier).handleAudioData(event);
    });

    gateway.on('VoiceSignal', (event) {
      final data = event['data'] as Map<String, dynamic>;
      final fromUserId = data['from_user_id'] as String;
      final signal = data['signal'] as Map<String, dynamic>;
      ref.read(videoProvider.notifier).handleSignal(fromUserId, signal);
    });

    gateway.onDisconnect(() {
      ref.read(presenceProvider.notifier).clear();
    });

    final authService = ref.read(authServiceProvider);
    final fileTransferService = FileTransferService(
      gateway: gateway,
      getToken: () => authService.token ?? '',
      getServerUrl: () => '${authService.serverUrl}/ft',
    );
    ref.read(fileTransferProvider.notifier).init(fileTransferService);

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
    ref.read(videoProvider.notifier).overlayContext = context;

    final activeChannel = ref.watch(activeChannelProvider);
    final videoState = ref.watch(videoProvider);
    final voiceState = ref.watch(voiceProvider);
    final myUserId = ref.watch(authProvider).userId;

    final isFullscreen = videoState.videoMode == VideoMode.fullscreen && videoState.hasAnyVideo;

    return Scaffold(
      body: Stack(
        children: [
          Row(
            children: [
              const Sidebar(),
              const VerticalDivider(width: 1, thickness: 1),
              Expanded(
                child: _buildContent(activeChannel),
              ),
            ],
          ),

          // Fullscreen video overlay
          if (isFullscreen)
            _FullscreenVideoOverlay(
              videoState: videoState,
              voiceState: voiceState,
              myUserId: myUserId,
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

class _FullscreenVideoOverlay extends ConsumerStatefulWidget {
  final VideoState videoState;
  final VoiceState voiceState;
  final String? myUserId;

  const _FullscreenVideoOverlay({
    required this.videoState,
    required this.voiceState,
    required this.myUserId,
  });

  @override
  ConsumerState<_FullscreenVideoOverlay> createState() => _FullscreenVideoOverlayState();
}

class _FullscreenVideoOverlayState extends ConsumerState<_FullscreenVideoOverlay> {
  final _focusNode = FocusNode();

  @override
  void initState() {
    super.initState();
    _focusNode.requestFocus();
  }

  @override
  void dispose() {
    _focusNode.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return KeyboardListener(
      focusNode: _focusNode,
      autofocus: true,
      onKeyEvent: (event) {
        if (event is KeyDownEvent && event.logicalKey == LogicalKeyboardKey.escape) {
          ref.read(videoProvider.notifier).setVideoMode(VideoMode.expanded);
        }
      },
      child: Container(
        color: Colors.black,
        child: Stack(
          children: [
            // Video grid fills entire screen
            Positioned.fill(
              child: _buildVideoGrid(),
            ),

            // Semi-transparent control bar at bottom
            Positioned(
              left: 0,
              right: 0,
              bottom: 0,
              child: Container(
                padding: const EdgeInsets.symmetric(horizontal: 24, vertical: 12),
                decoration: BoxDecoration(
                  gradient: LinearGradient(
                    begin: Alignment.bottomCenter,
                    end: Alignment.topCenter,
                    colors: [
                      Colors.black.withValues(alpha: 0.8),
                      Colors.transparent,
                    ],
                  ),
                ),
                child: Row(
                  mainAxisAlignment: MainAxisAlignment.center,
                  children: [
                    _fullscreenButton(
                      icon: widget.videoState.showOwnScreen ? Icons.visibility : Icons.visibility_off,
                      label: 'Preview',
                      visible: widget.videoState.screenShareEnabled,
                      onTap: () => ref.read(videoProvider.notifier).toggleShowOwnScreen(),
                    ),
                    const SizedBox(width: 24),
                    _fullscreenButton(
                      icon: Icons.fullscreen_exit,
                      label: 'Exit Fullscreen',
                      onTap: () => ref.read(videoProvider.notifier).setVideoMode(VideoMode.expanded),
                    ),
                  ],
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _fullscreenButton({
    required IconData icon,
    required String label,
    bool visible = true,
    required VoidCallback onTap,
  }) {
    if (!visible) return const SizedBox.shrink();
    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        Material(
          color: Colors.white.withValues(alpha: 0.15),
          shape: const CircleBorder(),
          child: InkWell(
            onTap: onTap,
            customBorder: const CircleBorder(),
            child: Padding(
              padding: const EdgeInsets.all(12),
              child: Icon(icon, color: Colors.white, size: 22),
            ),
          ),
        ),
        const SizedBox(height: 4),
        Text(label, style: const TextStyle(fontSize: 11, color: Colors.white70)),
      ],
    );
  }

  Widget _buildVideoGrid() {
    final tiles = collectVideoTiles(widget.videoState, widget.voiceState);
    return VideoGrid(
      tiles: tiles,
      focusedStreamId: widget.videoState.focusedStreamId,
      emptyText: 'No video',
    );
  }
}
