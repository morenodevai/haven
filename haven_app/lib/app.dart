import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import 'package:haven_app/config/theme.dart';
import 'package:haven_app/providers/auth_provider.dart';
import 'package:haven_app/screens/home_screen.dart';
import 'package:haven_app/screens/login_screen.dart';

/// Root widget — switches between login and home based on auth state.
/// Uses a simple conditional instead of GoRouter to avoid router recreation issues.
class HavenApp extends ConsumerWidget {
  const HavenApp({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final authState = ref.watch(authProvider);

    Widget body;
    switch (authState.status) {
      case AuthStatus.unknown:
        body = const Scaffold(
          body: Center(child: CircularProgressIndicator()),
        );
      case AuthStatus.unauthenticated:
        body = const LoginScreen();
      case AuthStatus.authenticated:
        body = const HomeScreen();
    }

    return MaterialApp(
      title: 'Haven',
      theme: HavenTheme.darkTheme,
      debugShowCheckedModeBanner: false,
      home: body,
    );
  }
}
