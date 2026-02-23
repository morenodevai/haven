import 'dart:async';
import 'dart:convert';

import 'package:flutter_secure_storage/flutter_secure_storage.dart';

import 'package:haven_app/config/constants.dart';
import 'package:haven_app/services/api_service.dart';

/// Authentication service — handles login, register, token storage, auto-refresh.
///
/// Tokens are stored in the OS keychain via flutter_secure_storage:
/// - Windows: Windows Credential Manager
/// - macOS: Keychain
/// - iOS: Keychain
/// - Android: EncryptedSharedPreferences
/// - Linux: libsecret
class AuthService {
  final ApiService _api;
  final FlutterSecureStorage _storage;
  Timer? _refreshTimer;

  String? _userId;
  String? _username;
  String? _token;

  AuthService(this._api)
      : _storage = const FlutterSecureStorage(
          aOptions: AndroidOptions(encryptedSharedPreferences: true),
        );

  String? get userId => _userId;
  String? get username => _username;
  String? get token => _token;
  bool get isLoggedIn => _token != null;

  /// Try to restore a saved session from secure storage.
  Future<bool> tryRestoreSession() async {
    try {
      final token = await _storage.read(key: 'haven_token');
      final userId = await _storage.read(key: 'haven_user_id');
      final username = await _storage.read(key: 'haven_username');
      final serverUrl = await _storage.read(key: 'haven_server_url');

      if (token == null || userId == null || username == null) {
        return false;
      }

      if (serverUrl != null) {
        _api.setBaseUrl(serverUrl);
      }

      // Check if token is expired
      if (_isTokenExpired(token)) {
        // Try to refresh
        _api.setToken(token);
        try {
          final result = await _api.refreshToken();
          final newToken = result['token'] as String;
          await _saveSession(userId, username, newToken);
          _api.setToken(newToken);
          _userId = userId;
          _username = username;
          _token = newToken;
          _startRefreshTimer();
          return true;
        } catch (_) {
          await clearSession();
          return false;
        }
      }

      _userId = userId;
      _username = username;
      _token = token;
      _api.setToken(token);
      _startRefreshTimer();
      return true;
    } catch (_) {
      return false;
    }
  }

  /// Register a new account.
  Future<void> register(String username, String password) async {
    final result = await _api.register(username, password);
    final userId = result['user_id'] as String;
    final token = result['token'] as String;

    await _saveSession(userId, username, token);
    _userId = userId;
    _username = username;
    _token = token;
    _api.setToken(token);
    _startRefreshTimer();
  }

  /// Login with existing credentials.
  Future<void> login(String username, String password) async {
    final result = await _api.login(username, password);
    final userId = result['user_id'] as String;
    final returnedUsername = result['username'] as String;
    final token = result['token'] as String;

    await _saveSession(userId, returnedUsername, token);
    _userId = userId;
    _username = returnedUsername;
    _token = token;
    _api.setToken(token);
    _startRefreshTimer();
  }

  /// Logout — clear all stored credentials.
  Future<void> logout() async {
    _refreshTimer?.cancel();
    _refreshTimer = null;
    _userId = null;
    _username = null;
    _token = null;
    _api.setToken(null);
    await clearSession();
  }

  /// Set the server URL.
  Future<void> setServerUrl(String url) async {
    _api.setBaseUrl(url);
    await _storage.write(key: 'haven_server_url', value: url);
  }

  /// Get the current server URL.
  String get serverUrl => _api.baseUrl;

  Future<void> _saveSession(
      String userId, String username, String token) async {
    await _storage.write(key: 'haven_token', value: token);
    await _storage.write(key: 'haven_user_id', value: userId);
    await _storage.write(key: 'haven_username', value: username);
  }

  Future<void> clearSession() async {
    await _storage.delete(key: 'haven_token');
    await _storage.delete(key: 'haven_user_id');
    await _storage.delete(key: 'haven_username');
  }

  bool _isTokenExpired(String token) {
    try {
      final parts = token.split('.');
      if (parts.length != 3) return true;

      // Decode JWT payload (base64url)
      String payload = parts[1];
      // Pad to multiple of 4
      switch (payload.length % 4) {
        case 2:
          payload += '==';
          break;
        case 3:
          payload += '=';
          break;
      }
      final decoded = utf8.decode(base64Url.decode(payload));
      final claims = jsonDecode(decoded) as Map<String, dynamic>;
      final exp = claims['exp'] as int;

      final expiry = DateTime.fromMillisecondsSinceEpoch(exp * 1000);
      final now = DateTime.now().toUtc();

      return now.isAfter(expiry);
    } catch (_) {
      return true;
    }
  }

  void _startRefreshTimer() {
    _refreshTimer?.cancel();

    if (_token == null) return;

    try {
      final parts = _token!.split('.');
      if (parts.length != 3) return;

      String payload = parts[1];
      switch (payload.length % 4) {
        case 2:
          payload += '==';
          break;
        case 3:
          payload += '=';
          break;
      }
      final decoded = utf8.decode(base64Url.decode(payload));
      final claims = jsonDecode(decoded) as Map<String, dynamic>;
      final exp = claims['exp'] as int;

      final expiry = DateTime.fromMillisecondsSinceEpoch(exp * 1000);
      final refreshAt =
          expiry.subtract(HavenConstants.tokenRefreshThreshold);
      final now = DateTime.now().toUtc();

      if (refreshAt.isAfter(now)) {
        final delay = refreshAt.difference(now);
        _refreshTimer = Timer(delay, _doRefresh);
      } else {
        // Token expires within the threshold — refresh now
        _doRefresh();
      }
    } catch (_) {
      // If we can't parse the token, try refreshing in 1 hour
      _refreshTimer = Timer(const Duration(hours: 1), _doRefresh);
    }
  }

  Future<void> _doRefresh() async {
    try {
      final result = await _api.refreshToken();
      final newToken = result['token'] as String;
      _token = newToken;
      _api.setToken(newToken);

      if (_userId != null && _username != null) {
        await _saveSession(_userId!, _username!, newToken);
      }

      _startRefreshTimer();
    } catch (_) {
      // Refresh failed — will try again at next interval
      _refreshTimer = Timer(const Duration(minutes: 5), _doRefresh);
    }
  }
}
