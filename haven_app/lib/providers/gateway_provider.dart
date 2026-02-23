import 'package:flutter_riverpod/flutter_riverpod.dart';

import 'package:haven_app/providers/auth_provider.dart';
import 'package:haven_app/services/gateway_service.dart';

/// Global gateway service — connects to WebSocket when authenticated.
final gatewayServiceProvider = Provider<GatewayService>((ref) {
  final authService = ref.watch(authServiceProvider);

  return GatewayService(
    getToken: () => authService.token ?? '',
    getBaseUrl: () => authService.serverUrl,
  );
});
