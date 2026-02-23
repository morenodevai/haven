import 'package:flutter/material.dart';

/// Haven 2.0 dark theme — matches the existing #1a1a2e dark palette.
class HavenTheme {
  HavenTheme._();

  // Core palette
  static const Color background = Color(0xFF1a1a2e);
  static const Color surface = Color(0xFF16213e);
  static const Color surfaceVariant = Color(0xFF0f3460);
  static const Color primary = Color(0xFF533483);
  static const Color primaryLight = Color(0xFF7c4dff);
  static const Color accent = Color(0xFFe94560);
  static const Color textPrimary = Color(0xFFffffff);
  static const Color textSecondary = Color(0xFFb0b0b0);
  static const Color textMuted = Color(0xFF6c6c6c);
  static const Color divider = Color(0xFF2a2a4a);
  static const Color online = Color(0xFF4caf50);
  static const Color error = Color(0xFFe94560);
  static const Color inputBackground = Color(0xFF0f0f23);
  static const Color messageBubbleSelf = Color(0xFF533483);
  static const Color messageBubbleOther = Color(0xFF16213e);
  static const Color sidebarBackground = Color(0xFF0f0f23);

  static ThemeData get darkTheme {
    return ThemeData(
      brightness: Brightness.dark,
      scaffoldBackgroundColor: background,
      primaryColor: primary,
      colorScheme: const ColorScheme.dark(
        primary: primaryLight,
        secondary: accent,
        surface: surface,
        error: error,
        onPrimary: textPrimary,
        onSecondary: textPrimary,
        onSurface: textPrimary,
        onError: textPrimary,
      ),
      appBarTheme: const AppBarTheme(
        backgroundColor: surface,
        foregroundColor: textPrimary,
        elevation: 0,
      ),
      cardTheme: const CardThemeData(
        color: surface,
        elevation: 2,
      ),
      dividerTheme: const DividerThemeData(
        color: divider,
        thickness: 1,
      ),
      inputDecorationTheme: InputDecorationTheme(
        filled: true,
        fillColor: inputBackground,
        border: OutlineInputBorder(
          borderRadius: BorderRadius.circular(12),
          borderSide: BorderSide.none,
        ),
        hintStyle: const TextStyle(color: textMuted),
        contentPadding:
            const EdgeInsets.symmetric(horizontal: 16, vertical: 14),
      ),
      elevatedButtonTheme: ElevatedButtonThemeData(
        style: ElevatedButton.styleFrom(
          backgroundColor: primaryLight,
          foregroundColor: textPrimary,
          padding: const EdgeInsets.symmetric(horizontal: 32, vertical: 14),
          shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(12),
          ),
          textStyle: const TextStyle(
            fontSize: 16,
            fontWeight: FontWeight.w600,
          ),
        ),
      ),
      textButtonTheme: TextButtonThemeData(
        style: TextButton.styleFrom(
          foregroundColor: primaryLight,
        ),
      ),
      iconTheme: const IconThemeData(
        color: textSecondary,
      ),
      textTheme: const TextTheme(
        headlineLarge: TextStyle(
          color: textPrimary,
          fontSize: 28,
          fontWeight: FontWeight.bold,
        ),
        headlineMedium: TextStyle(
          color: textPrimary,
          fontSize: 22,
          fontWeight: FontWeight.w600,
        ),
        bodyLarge: TextStyle(
          color: textPrimary,
          fontSize: 16,
        ),
        bodyMedium: TextStyle(
          color: textSecondary,
          fontSize: 14,
        ),
        labelLarge: TextStyle(
          color: textPrimary,
          fontSize: 14,
          fontWeight: FontWeight.w500,
        ),
      ),
      snackBarTheme: SnackBarThemeData(
        backgroundColor: surface,
        contentTextStyle: const TextStyle(color: textPrimary),
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(8),
        ),
        behavior: SnackBarBehavior.floating,
      ),
    );
  }
}
