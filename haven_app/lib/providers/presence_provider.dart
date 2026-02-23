import 'package:flutter_riverpod/flutter_riverpod.dart';

import 'package:haven_app/models/user.dart';

/// Tracks online users via PresenceUpdate gateway events.
class PresenceNotifier extends StateNotifier<Map<String, User>> {
  PresenceNotifier() : super({});

  void userOnline(String userId, String username) {
    state = {
      ...state,
      userId: User(id: userId, username: username, online: true),
    };
  }

  void userOffline(String userId) {
    final updated = Map<String, User>.from(state);
    updated.remove(userId);
    state = updated;
  }

  void clear() {
    state = {};
  }

  bool isOnline(String userId) => state.containsKey(userId);

  List<User> get onlineUsers => state.values.toList();
}

final presenceProvider =
    StateNotifierProvider<PresenceNotifier, Map<String, User>>((ref) {
  return PresenceNotifier();
});
