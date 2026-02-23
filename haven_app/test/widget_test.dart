import 'package:flutter_test/flutter_test.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import 'package:haven_app/app.dart';

void main() {
  testWidgets('Haven app launches with login screen', (WidgetTester tester) async {
    await tester.pumpWidget(
      const ProviderScope(child: HavenApp()),
    );
    await tester.pumpAndSettle();

    // Should show Haven title on login
    expect(find.text('Haven'), findsWidgets);
  });
}
