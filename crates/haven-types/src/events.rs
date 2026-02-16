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
        ciphertext: Vec<u8>,
        nonce: Vec<u8>,
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
}

/// Commands sent FROM client TO server over WebSocket.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum GatewayCommand {
    /// Authenticate the WebSocket connection
    Identify { token: String },

    /// Indicate typing in a channel
    StartTyping { channel_id: Uuid },
}
