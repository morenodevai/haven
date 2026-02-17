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
