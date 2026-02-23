import 'package:intl/intl.dart';

/// Format a file size into human-readable string.
String formatFileSize(int bytes) {
  if (bytes < 1024) return '$bytes B';
  if (bytes < 1024 * 1024) return '${(bytes / 1024).toStringAsFixed(1)} KB';
  if (bytes < 1024 * 1024 * 1024) {
    return '${(bytes / (1024 * 1024)).toStringAsFixed(1)} MB';
  }
  return '${(bytes / (1024 * 1024 * 1024)).toStringAsFixed(2)} GB';
}

/// Format a transfer speed.
String formatSpeed(double bytesPerSecond) {
  return '${formatFileSize(bytesPerSecond.toInt())}/s';
}

/// Format an ISO timestamp for display.
String formatTimestamp(String isoTimestamp) {
  try {
    final dt = DateTime.parse(isoTimestamp).toLocal();
    final now = DateTime.now();
    final today = DateTime(now.year, now.month, now.day);
    final messageDate = DateTime(dt.year, dt.month, dt.day);

    if (messageDate == today) {
      return DateFormat('h:mm a').format(dt);
    } else if (messageDate == today.subtract(const Duration(days: 1))) {
      return 'Yesterday ${DateFormat('h:mm a').format(dt)}';
    } else {
      return DateFormat('MMM d, h:mm a').format(dt);
    }
  } catch (_) {
    return isoTimestamp;
  }
}
