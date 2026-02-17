use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Events sent over the WebSocket gateway.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum GatewayEvent {
    /// Server confirms successful authentication
    Ready { user_id: Uuid, username: String },

    /// A new encrypted message was posted
    MessageCreate {
        id: Uuid,
        channel_id: Uuid,
        author_id: Uuid,
        author_username: String,
        ciphertext: String,
        nonce: String,
        timestamp: chrono::DateTime<chrono::Utc>,
    },

    /// A user started typing
    TypingStart {
        channel_id: Uuid,
        user_id: Uuid,
        username: String,
    },

    /// A user came online or went offline
    PresenceUpdate {
        user_id: Uuid,
        username: String,
        online: bool,
    },

    /// A reaction was added to a message
    ReactionAdd {
        message_id: Uuid,
        user_id: Uuid,
        username: String,
        emoji: String,
    },

    /// A reaction was removed from a message
    ReactionRemove {
        message_id: Uuid,
        user_id: Uuid,
        emoji: String,
    },

    /// Voice channel state update (join/leave/mute/deafen)
    VoiceStateUpdate {
        channel_id: Uuid,
        user_id: Uuid,
        username: String,
        session_id: Option<String>,
        self_mute: bool,
        self_deaf: bool,
    },

    /// Voice signaling message targeted to a specific user
    VoiceSignal {
        from_user_id: Uuid,
        signal: VoiceSignalPayload,
    },

    /// Server-relayed voice audio data
    VoiceAudioData {
        from_user_id: Uuid,
        data: String,
    },
}

impl GatewayEvent {
    /// Returns the channel_id if this event is scoped to a specific channel.
    /// Events that return `None` are global and should be delivered to all clients.
    pub fn channel_id(&self) -> Option<Uuid> {
        match self {
            Self::MessageCreate { channel_id, .. } => Some(*channel_id),
            Self::TypingStart { channel_id, .. } => Some(*channel_id),
            Self::VoiceStateUpdate { channel_id, .. } => Some(*channel_id),
            // Ready, PresenceUpdate, ReactionAdd/Remove, VoiceSignal, VoiceAudioData are global
            _ => None,
        }
    }
}

/// Commands sent FROM client TO server over WebSocket.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum GatewayCommand {
    /// Authenticate the WebSocket connection
    Identify { token: String },

    /// Indicate typing in a channel
    StartTyping { channel_id: Uuid },

    /// Join a voice channel
    VoiceJoin { channel_id: Uuid },

    /// Leave the current voice channel
    VoiceLeave,

    /// Update self-mute/deafen state
    VoiceStateSet { self_mute: bool, self_deaf: bool },

    /// Send a voice signaling message to a specific peer
    VoiceSignalSend {
        target_user_id: Uuid,
        signal: VoiceSignalPayload,
    },

    /// Send voice audio data to be relayed to other participants
    VoiceData { data: String },

    /// Subscribe to events for specific channels.
    /// The server will only forward channel-scoped events (messages, typing, voice)
    /// for channels the client has subscribed to.
    Subscribe { channel_ids: Vec<Uuid> },
}

/// WebRTC signaling payload relayed between peers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "signal_type")]
pub enum VoiceSignalPayload {
    Offer { sdp: String },
    Answer { sdp: String },
    IceCandidate {
        candidate: String,
        sdp_mid: Option<String>,
        sdp_m_line_index: Option<u16>,
    },
}
