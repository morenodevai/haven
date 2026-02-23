import 'package:flutter_riverpod/flutter_riverpod.dart';

import 'package:haven_app/services/api_service.dart';
import 'package:haven_app/services/auth_service.dart';

/// Global API service instance.
final apiServiceProvider = Provider<ApiService>((ref) {
  return ApiService();
});

/// Global auth service instance.
final authServiceProvider = Provider<AuthService>((ref) {
  final api = ref.watch(apiServiceProvider);
  return AuthService(api);
});

/// Auth state — tracks whether the user is logged in.
enum AuthStatus { unknown, authenticated, unauthenticated }

class AuthState {
  final AuthStatus status;
  final String? userId;
  final String? username;
  final String? error;
  final bool isLoading;

  const AuthState({
    this.status = AuthStatus.unknown,
    this.userId,
    this.username,
    this.error,
    this.isLoading = false,
  });

  AuthState copyWith({
    AuthStatus? status,
    String? userId,
    String? username,
    String? error,
    bool? isLoading,
  }) {
    return AuthState(
      status: status ?? this.status,
      userId: userId ?? this.userId,
      username: username ?? this.username,
      error: error,
      isLoading: isLoading ?? this.isLoading,
    );
  }
}

class AuthNotifier extends StateNotifier<AuthState> {
  final AuthService _authService;

  AuthNotifier(this._authService) : super(const AuthState());

  Future<void> init() async {
    final restored = await _authService.tryRestoreSession();
    if (restored) {
      state = AuthState(
        status: AuthStatus.authenticated,
        userId: _authService.userId,
        username: _authService.username,
      );
    } else {
      state = const AuthState(status: AuthStatus.unauthenticated);
    }
  }

  Future<void> login(String username, String password) async {
    state = state.copyWith(isLoading: true, error: null);
    try {
      await _authService.login(username, password);
      state = AuthState(
        status: AuthStatus.authenticated,
        userId: _authService.userId,
        username: _authService.username,
      );
    } catch (e) {
      String message = 'Login failed';
      if (e.toString().contains('401')) {
        message = 'Invalid username or password';
      } else if (e.toString().contains('429')) {
        message = 'Too many attempts. Please wait.';
      } else if (e.toString().contains('SocketException') ||
          e.toString().contains('Connection refused')) {
        message = 'Cannot connect to server';
      }
      state = state.copyWith(isLoading: false, error: message);
    }
  }

  Future<void> register(String username, String password) async {
    state = state.copyWith(isLoading: true, error: null);
    try {
      await _authService.register(username, password);
      state = AuthState(
        status: AuthStatus.authenticated,
        userId: _authService.userId,
        username: _authService.username,
      );
    } catch (e) {
      String message = 'Registration failed';
      if (e.toString().contains('409')) {
        message = 'Username already taken';
      } else if (e.toString().contains('400')) {
        message = 'Invalid username or password format';
      } else if (e.toString().contains('429')) {
        message = 'Too many attempts. Please wait.';
      } else if (e.toString().contains('SocketException') ||
          e.toString().contains('Connection refused')) {
        message = 'Cannot connect to server';
      }
      state = state.copyWith(isLoading: false, error: message);
    }
  }

  Future<void> logout() async {
    await _authService.logout();
    state = const AuthState(status: AuthStatus.unauthenticated);
  }

  Future<void> setServerUrl(String url) async {
    await _authService.setServerUrl(url);
  }

  String get serverUrl => _authService.serverUrl;
}

final authProvider = StateNotifierProvider<AuthNotifier, AuthState>((ref) {
  final authService = ref.watch(authServiceProvider);
  return AuthNotifier(authService);
});
