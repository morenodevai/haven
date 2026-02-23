import 'package:flutter_riverpod/flutter_riverpod.dart';

import 'package:haven_app/models/channel.dart';

/// Active channel selection.
final activeChannelProvider = StateProvider<Channel>((ref) {
  return Channel.defaults.first; // general
});

/// All available channels.
final channelsProvider = Provider<List<Channel>>((ref) {
  return Channel.defaults;
});
