use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: Uuid,
    pub username: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Channel {
    pub id: Uuid,
    pub name: String,
    pub created_at: DateTime<Utc>,
}

/// Messages stored on the server are always encrypted.
/// The server only sees ciphertext — never plaintext.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedMessage {
    pub id: Uuid,
    pub channel_id: Uuid,
    pub author_id: Uuid,
    pub ciphertext: Vec<u8>,
    pub nonce: Vec<u8>,
    pub created_at: DateTime<Utc>,
}
