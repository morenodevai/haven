import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import 'package:haven_app/config/constants.dart';
import 'package:haven_app/config/theme.dart';
import 'package:haven_app/providers/auth_provider.dart';

class LoginScreen extends ConsumerStatefulWidget {
  const LoginScreen({super.key});

  @override
  ConsumerState<LoginScreen> createState() => _LoginScreenState();
}

class _LoginScreenState extends ConsumerState<LoginScreen> {
  final _usernameController = TextEditingController();
  final _passwordController = TextEditingController();
  final _serverUrlController = TextEditingController();
  bool _isRegisterMode = false;
  bool _showServerConfig = false;
  bool _obscurePassword = true;

  @override
  void initState() {
    super.initState();
    final authNotifier = ref.read(authProvider.notifier);
    _serverUrlController.text = authNotifier.serverUrl;
  }

  @override
  void dispose() {
    _usernameController.dispose();
    _passwordController.dispose();
    _serverUrlController.dispose();
    super.dispose();
  }

  Future<void> _submit() async {
    final username = _usernameController.text.trim();
    final password = _passwordController.text;

    if (username.isEmpty || password.isEmpty) return;

    // Validate
    if (username.length < 3 || username.length > 32) {
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(content: Text('Username must be 3-32 characters')),
      );
      return;
    }
    if (password.length < 8) {
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(content: Text('Password must be at least 8 characters')),
      );
      return;
    }

    // Apply server URL if changed
    final serverUrl = _serverUrlController.text.trim();
    if (serverUrl.isNotEmpty && serverUrl != HavenConstants.defaultServerUrl) {
      await ref.read(authProvider.notifier).setServerUrl(serverUrl);
    }

    if (_isRegisterMode) {
      await ref.read(authProvider.notifier).register(username, password);
    } else {
      await ref.read(authProvider.notifier).login(username, password);
    }
  }

  @override
  Widget build(BuildContext context) {
    final authState = ref.watch(authProvider);

    return Scaffold(
      body: Center(
        child: SingleChildScrollView(
          padding: const EdgeInsets.all(32),
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 400),
            child: Column(
              mainAxisAlignment: MainAxisAlignment.center,
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                // Logo / Title
                Icon(
                  Icons.shield_outlined,
                  size: 64,
                  color: HavenTheme.primaryLight,
                ),
                const SizedBox(height: 16),
                Text(
                  'Haven',
                  style: Theme.of(context).textTheme.headlineLarge,
                  textAlign: TextAlign.center,
                ),
                const SizedBox(height: 4),
                Text(
                  'v2.0',
                  style: Theme.of(context).textTheme.bodyMedium,
                  textAlign: TextAlign.center,
                ),
                const SizedBox(height: 40),

                // Error message
                if (authState.error != null) ...[
                  Container(
                    padding: const EdgeInsets.all(12),
                    decoration: BoxDecoration(
                      color: HavenTheme.error.withValues(alpha: 0.15),
                      borderRadius: BorderRadius.circular(8),
                    ),
                    child: Text(
                      authState.error!,
                      style: const TextStyle(color: HavenTheme.error),
                      textAlign: TextAlign.center,
                    ),
                  ),
                  const SizedBox(height: 16),
                ],

                // Username
                TextField(
                  controller: _usernameController,
                  decoration: const InputDecoration(
                    hintText: 'Username',
                    prefixIcon: Icon(Icons.person_outline),
                  ),
                  textInputAction: TextInputAction.next,
                  autocorrect: false,
                  autofocus: true,
                ),
                const SizedBox(height: 12),

                // Password
                TextField(
                  controller: _passwordController,
                  decoration: InputDecoration(
                    hintText: 'Password',
                    prefixIcon: const Icon(Icons.lock_outline),
                    suffixIcon: IconButton(
                      icon: Icon(
                        _obscurePassword
                            ? Icons.visibility_off
                            : Icons.visibility,
                      ),
                      onPressed: () {
                        setState(() {
                          _obscurePassword = !_obscurePassword;
                        });
                      },
                    ),
                  ),
                  obscureText: _obscurePassword,
                  textInputAction: TextInputAction.done,
                  onSubmitted: (_) => _submit(),
                ),
                const SizedBox(height: 24),

                // Submit button
                ElevatedButton(
                  onPressed: authState.isLoading ? null : _submit,
                  child: authState.isLoading
                      ? const SizedBox(
                          height: 20,
                          width: 20,
                          child: CircularProgressIndicator(
                            strokeWidth: 2,
                            color: Colors.white,
                          ),
                        )
                      : Text(_isRegisterMode ? 'Create Account' : 'Sign In'),
                ),
                const SizedBox(height: 12),

                // Toggle login/register
                TextButton(
                  onPressed: () {
                    setState(() {
                      _isRegisterMode = !_isRegisterMode;
                    });
                  },
                  child: Text(
                    _isRegisterMode
                        ? 'Already have an account? Sign In'
                        : 'Need an account? Register',
                  ),
                ),

                const SizedBox(height: 16),

                // Server config toggle
                TextButton.icon(
                  onPressed: () {
                    setState(() {
                      _showServerConfig = !_showServerConfig;
                    });
                  },
                  icon: Icon(
                    _showServerConfig
                        ? Icons.expand_less
                        : Icons.expand_more,
                    size: 18,
                  ),
                  label: Text(
                    'Server Settings',
                    style: TextStyle(
                      color: HavenTheme.textMuted,
                      fontSize: 13,
                    ),
                  ),
                ),

                if (_showServerConfig) ...[
                  const SizedBox(height: 8),
                  TextField(
                    controller: _serverUrlController,
                    decoration: const InputDecoration(
                      hintText: 'Server URL',
                      prefixIcon: Icon(Icons.dns_outlined),
                    ),
                    textInputAction: TextInputAction.done,
                    style: const TextStyle(fontSize: 14),
                  ),
                ],
              ],
            ),
          ),
        ),
      ),
    );
  }
}
