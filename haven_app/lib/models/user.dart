class User {
  final String id;
  final String username;
  final bool online;

  const User({
    required this.id,
    required this.username,
    this.online = false,
  });

  User copyWith({String? id, String? username, bool? online}) {
    return User(
      id: id ?? this.id,
      username: username ?? this.username,
      online: online ?? this.online,
    );
  }
}
