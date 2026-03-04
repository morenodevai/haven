import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_webrtc/flutter_webrtc.dart';

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
import 'package:haven_app/services/webrtc_service.dart';
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

class _FullscreenVideoOverlay extends ConsumerWidget {
  final VideoState videoState;
  final VoiceState voiceState;
  final String? myUserId;

  const _FullscreenVideoOverlay({
    required this.videoState,
    required this.voiceState,
    required this.myUserId,
  });

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    return KeyboardListener(
      focusNode: FocusNode()..requestFocus(),
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
              child: _buildVideoGrid(ref),
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
                      icon: videoState.showOwnScreen ? Icons.visibility : Icons.visibility_off,
                      label: 'Preview',
                      visible: videoState.screenShareEnabled,
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

  Widget _buildVideoGrid(WidgetRef ref) {
    final tiles = _collectTiles(ref);
    if (tiles.isEmpty) {
      return const Center(
        child: Text('No video', style: TextStyle(color: Colors.white54)),
      );
    }

    final focusedId = videoState.focusedStreamId;
    if (focusedId != null) {
      final focusedIdx = tiles.indexWhere((t) => t.id == focusedId);
      if (focusedIdx >= 0) {
        return _buildFocusedLayout(tiles, focusedIdx, ref);
      }
    }

    return _buildEqualGrid(tiles, ref);
  }

  List<_VideoTileData> _collectTiles(WidgetRef ref) {
    final tiles = <_VideoTileData>[];

    // Local camera
    if (videoState.cameraEnabled && videoState.localCameraRenderer != null) {
      tiles.add(_VideoTileData(
        id: 'local-camera',
        renderer: videoState.localCameraRenderer!,
        label: 'You',
        mirror: true,
        isScreen: false,
      ));
    }

    // Local screen (only if showOwnScreen)
    if (videoState.screenShareEnabled && videoState.showOwnScreen && videoState.localScreenRenderer != null) {
      tiles.add(_VideoTileData(
        id: 'local-screen',
        renderer: videoState.localScreenRenderer!,
        label: 'You (Screen)',
        isScreen: true,
      ));
    }

    // Remote streams
    for (final entry in videoState.remoteStreams.entries) {
      final peerId = entry.key;
      for (final rs in entry.value) {
        final participant = voiceState.participants[peerId];
        final name = participant?.username ?? peerId.substring(0, 8);
        tiles.add(_VideoTileData(
          id: '$peerId-${rs.stream.id}',
          renderer: rs.renderer,
          label: rs.kind == VideoTrackKind.screen ? '$name (Screen)' : name,
          isScreen: rs.kind == VideoTrackKind.screen,
        ));
      }
    }

    return tiles;
  }

  Widget _buildEqualGrid(List<_VideoTileData> tiles, WidgetRef ref) {
    final count = tiles.length;
    if (count == 1) return _tile(tiles[0], ref);
    if (count == 2) {
      return Row(
        children: tiles.map((t) => Expanded(child: _tile(t, ref))).toList(),
      );
    }
    if (count <= 4) {
      return Column(
        children: [
          Expanded(
            child: Row(
              children: tiles.take(2).map((t) => Expanded(child: _tile(t, ref))).toList(),
            ),
          ),
          Expanded(
            child: Row(
              children: tiles.skip(2).map((t) => Expanded(child: _tile(t, ref))).toList(),
            ),
          ),
        ],
      );
    }
    // 5-6: 3x2
    return Column(
      children: [
        Expanded(
          child: Row(
            children: tiles.take(3).map((t) => Expanded(child: _tile(t, ref))).toList(),
          ),
        ),
        Expanded(
          child: Row(
            children: tiles.skip(3).map((t) => Expanded(child: _tile(t, ref))).toList(),
          ),
        ),
      ],
    );
  }

  Widget _buildFocusedLayout(List<_VideoTileData> tiles, int focusedIdx, WidgetRef ref) {
    final focused = tiles[focusedIdx];
    final others = [...tiles]..removeAt(focusedIdx);

    return Row(
      children: [
        // Focused tile ~75%
        Expanded(
          flex: 3,
          child: _tile(focused, ref),
        ),
        // Others strip
        if (others.isNotEmpty)
          SizedBox(
            width: 180,
            child: ListView(
              children: others.map((t) => SizedBox(
                height: 120,
                child: _tile(t, ref),
              )).toList(),
            ),
          ),
      ],
    );
  }

  Widget _tile(_VideoTileData data, WidgetRef ref) {
    return GestureDetector(
      onTap: () => ref.read(videoProvider.notifier).setFocusedStream(data.id),
      child: Container(
        color: Colors.black,
        child: Stack(
          fit: StackFit.expand,
          children: [
            RTCVideoView(
              data.renderer,
              mirror: data.mirror,
              objectFit: data.isScreen
                  ? RTCVideoViewObjectFit.RTCVideoViewObjectFitContain
                  : RTCVideoViewObjectFit.RTCVideoViewObjectFitCover,
            ),
            Positioned(
              left: 4,
              bottom: 4,
              child: Container(
                padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 2),
                decoration: BoxDecoration(
                  color: Colors.black54,
                  borderRadius: BorderRadius.circular(4),
                ),
                child: Text(
                  data.label,
                  style: const TextStyle(
                    color: Colors.white,
                    fontSize: 11,
                    fontWeight: FontWeight.w500,
                  ),
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _VideoTileData {
  final String id;
  final RTCVideoRenderer renderer;
  final String label;
  final bool mirror;
  final bool isScreen;

  _VideoTileData({
    required this.id,
    required this.renderer,
    required this.label,
    this.mirror = false,
    this.isScreen = false,
  });
}
