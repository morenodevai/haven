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

    /// A peer is offering to send a file
    FileOffer {
        from_user_id: Uuid,
        transfer_id: String,
        filename: String,
        size: u64,
    },

    /// A peer accepted a file transfer
    FileAccept {
        from_user_id: Uuid,
        transfer_id: String,
    },

    /// A peer rejected a file transfer
    FileReject {
        from_user_id: Uuid,
        transfer_id: String,
    },

    /// WebRTC signaling for a file transfer (SDP offer/answer, ICE candidates)
    FileSignal {
        from_user_id: Uuid,
        transfer_id: String,
        signal: VoiceSignalPayload,
    },

    /// Server-relayed file chunk (fallback when P2P fails)
    FileChunk {
        from_user_id: Uuid,
        transfer_id: String,
        chunk_index: u64,
        data: String, // base64-encoded encrypted chunk
    },

    /// Server-relayed file transfer complete signal
    FileDone {
        from_user_id: Uuid,
        transfer_id: String,
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

    /// Offer to send a file to a specific peer
    FileOfferSend {
        target_user_id: Uuid,
        transfer_id: String,
        filename: String,
        size: u64,
    },

    /// Accept a file transfer from a peer
    FileAcceptSend {
        target_user_id: Uuid,
        transfer_id: String,
    },

    /// Reject a file transfer from a peer
    FileRejectSend {
        target_user_id: Uuid,
        transfer_id: String,
    },

    /// Send WebRTC signaling for a file transfer to a specific peer
    FileSignalSend {
        target_user_id: Uuid,
        transfer_id: String,
        signal: VoiceSignalPayload,
    },

    /// Relay a file chunk through the server (fallback when P2P fails)
    FileChunkSend {
        target_user_id: Uuid,
        transfer_id: String,
        chunk_index: u64,
        data: String, // base64-encoded encrypted chunk
    },

    /// Signal file transfer complete via relay
    FileDoneSend {
        target_user_id: Uuid,
        transfer_id: String,
    },
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
