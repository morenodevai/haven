use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

// -- JWT Claims --

/// JWT claims shared across haven-api (REST middleware) and haven-gateway
/// (WebSocket authentication). Canonical definition lives here in haven-types
/// to eliminate duplication.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: Uuid,
    pub username: String,
    pub exp: usize,
}

// -- Auth --

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct RegisterResponse {
    pub user_id: Uuid,
    pub token: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct LoginResponse {
    pub user_id: Uuid,
    pub username: String,
    pub token: String,
}

// -- Messages --

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SendMessageRequest {
    pub ciphertext: String,
    pub nonce: String,
}

#[derive(Debug, Serialize)]
pub struct MessageResponse {
    pub id: Uuid,
    pub channel_id: Uuid,
    pub author_id: Uuid,
    pub author_username: String,
    pub ciphertext: String,
    pub nonce: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub reactions: Vec<ReactionGroup>,
}

// -- Reactions --

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToggleReactionRequest {
    pub emoji: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReactionGroup {
    pub emoji: String,
    pub count: usize,
    pub user_ids: Vec<Uuid>,
}

// -- Status enums --

/// Status of a file transfer on the file server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransferStatus {
    Uploading,
    Complete,
    Confirmed,
    Expired,
}

impl fmt::Display for TransferStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Uploading => write!(f, "uploading"),
            Self::Complete => write!(f, "complete"),
            Self::Confirmed => write!(f, "confirmed"),
            Self::Expired => write!(f, "expired"),
        }
    }
}

impl FromStr for TransferStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "uploading" => Ok(Self::Uploading),
            "complete" => Ok(Self::Complete),
            "confirmed" => Ok(Self::Confirmed),
            "expired" => Ok(Self::Expired),
            other => Err(format!("unknown transfer status: {}", other)),
        }
    }
}

/// Status of a file/folder offer on the gateway.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OfferStatus {
    Pending,
    Accepted,
    Rejected,
    Uploaded,
}

impl fmt::Display for OfferStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Accepted => write!(f, "accepted"),
            Self::Rejected => write!(f, "rejected"),
            Self::Uploaded => write!(f, "uploaded"),
        }
    }
}

impl FromStr for OfferStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending" => Ok(Self::Pending),
            "accepted" => Ok(Self::Accepted),
            "rejected" => Ok(Self::Rejected),
            "uploaded" => Ok(Self::Uploaded),
            other => Err(format!("unknown offer status: {}", other)),
        }
    }
}
