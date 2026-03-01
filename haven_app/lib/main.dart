import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:package_info_plus/package_info_plus.dart';

import 'package:haven_app/app.dart';
import 'package:haven_app/config/constants.dart';
import 'package:haven_app/providers/auth_provider.dart';

void main() async {
  WidgetsFlutterBinding.ensureInitialized();

  // Read version from pubspec.yaml automatically
  final packageInfo = await PackageInfo.fromPlatform();
  appVersion = 'v${packageInfo.version}';

  final container = ProviderContainer();

  // Initialize auth — try to restore saved session
  await container.read(authProvider.notifier).init();

  runApp(
    UncontrolledProviderScope(
      container: container,
      child: const HavenApp(),
    ),
  );
}
