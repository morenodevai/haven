import 'dart:async';

import 'package:flutter_riverpod/flutter_riverpod.dart';

import 'package:haven_app/config/constants.dart';

/// Tracks who is currently typing.
class TypingNotifier extends StateNotifier<Map<String, String>> {
  final Map<String, Timer> _timers = {};

  TypingNotifier() : super({});

  void userTyping(String userId, String username) {
    // Cancel existing timer for this user
    _timers[userId]?.cancel();

    // Add to typing set
    state = {...state, userId: username};

    // Remove after timeout
    _timers[userId] = Timer(HavenConstants.typingTimeout, () {
      userStoppedTyping(userId);
    });
  }

  void userStoppedTyping(String userId) {
    _timers[userId]?.cancel();
    _timers.remove(userId);
    final updated = Map<String, String>.from(state);
    updated.remove(userId);
    state = updated;
  }

  void clear() {
    for (final timer in _timers.values) {
      timer.cancel();
    }
    _timers.clear();
    state = {};
  }

  @override
  void dispose() {
    clear();
    super.dispose();
  }
}

final typingProvider =
    StateNotifierProvider<TypingNotifier, Map<String, String>>((ref) {
  return TypingNotifier();
});
