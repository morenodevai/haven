import 'package:haven_app/config/constants.dart';

enum ChannelType { text, voice, file }

class Channel {
  final String id;
  final String name;
  final ChannelType type;

  const Channel({
    required this.id,
    required this.name,
    required this.type,
  });

  static const List<Channel> defaults = [
    Channel(
      id: HavenConstants.generalChannelId,
      name: 'general',
      type: ChannelType.text,
    ),
    Channel(
      id: HavenConstants.voiceChannelId,
      name: 'voice',
      type: ChannelType.voice,
    ),
    Channel(
      id: HavenConstants.fileChannelId,
      name: 'file-sharing',
      type: ChannelType.file,
    ),
  ];
}
