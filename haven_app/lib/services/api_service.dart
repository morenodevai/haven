import 'dart:typed_data';

import 'package:dio/dio.dart';

import 'package:haven_app/config/constants.dart';

/// REST API client for the Haven server.
///
/// Handles all HTTP endpoints:
/// - POST /auth/register
/// - POST /auth/login
/// - POST /auth/refresh
/// - GET  /channels/{channelId}/messages
/// - POST /channels/{channelId}/messages
/// - POST /channels/{channelId}/messages/{messageId}/reactions
/// - POST /files
/// - GET  /files/{fileId}
class ApiService {
  late final Dio _dio;
  String _baseUrl;
  String? _token;

  ApiService({String? baseUrl})
      : _baseUrl = baseUrl ?? HavenConstants.defaultServerUrl {
    _dio = Dio(BaseOptions(
      baseUrl: _baseUrl,
      connectTimeout: const Duration(seconds: 10),
      receiveTimeout: const Duration(seconds: 30),
      sendTimeout: const Duration(seconds: 60),
    ));

    _dio.interceptors.add(InterceptorsWrapper(
      onRequest: (options, handler) {
        if (_token != null) {
          options.headers['Authorization'] = 'Bearer $_token';
        }
        return handler.next(options);
      },
    ));
  }

  String get baseUrl => _baseUrl;

  void setBaseUrl(String url) {
    _baseUrl = url;
    _dio.options.baseUrl = url;
  }

  void setToken(String? token) {
    _token = token;
  }

  // -- Auth --

  Future<Map<String, dynamic>> register(
      String username, String password) async {
    final response = await _dio.post('/auth/register', data: {
      'username': username,
      'password': password,
    });
    return response.data as Map<String, dynamic>;
  }

  Future<Map<String, dynamic>> login(
      String username, String password) async {
    final response = await _dio.post('/auth/login', data: {
      'username': username,
      'password': password,
    });
    return response.data as Map<String, dynamic>;
  }

  Future<Map<String, dynamic>> refreshToken() async {
    final response = await _dio.post('/auth/refresh');
    return response.data as Map<String, dynamic>;
  }

  // -- Messages --

  Future<List<dynamic>> getMessages(
    String channelId, {
    int limit = 50,
    String? before,
  }) async {
    final queryParams = <String, dynamic>{'limit': limit};
    if (before != null) {
      queryParams['before'] = before;
    }
    final response = await _dio.get(
      '/channels/$channelId/messages',
      queryParameters: queryParams,
    );
    return response.data as List<dynamic>;
  }

  Future<Map<String, dynamic>> sendMessage(
    String channelId,
    String ciphertext,
    String nonce,
  ) async {
    final response = await _dio.post(
      '/channels/$channelId/messages',
      data: {
        'ciphertext': ciphertext,
        'nonce': nonce,
      },
    );
    return response.data as Map<String, dynamic>;
  }

  // -- Reactions --

  Future<Map<String, dynamic>> toggleReaction(
    String channelId,
    String messageId,
    String emoji,
  ) async {
    final response = await _dio.post(
      '/channels/$channelId/messages/$messageId/reactions',
      data: {'emoji': emoji},
    );
    return response.data as Map<String, dynamic>;
  }

  // -- Files --

  Future<Map<String, dynamic>> uploadFile(Uint8List encryptedBytes) async {
    final response = await _dio.post(
      '/files',
      data: Stream.fromIterable([encryptedBytes]),
      options: Options(
        headers: {
          'Content-Type': 'application/octet-stream',
          'Content-Length': encryptedBytes.length,
        },
      ),
    );
    return response.data as Map<String, dynamic>;
  }

  Future<Uint8List> downloadFile(String fileId) async {
    final response = await _dio.get<List<int>>(
      '/files/$fileId',
      options: Options(responseType: ResponseType.bytes),
    );
    return Uint8List.fromList(response.data!);
  }
}
