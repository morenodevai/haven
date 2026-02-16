use serde::{Deserialize, Serialize};
use uuid::Uuid;

// -- Auth --

#[derive(Debug, Deserialize)]
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
pub struct SendMessageRequest {
    pub ciphertext: Vec<u8>,
    pub nonce: Vec<u8>,
}

#[derive(Debug, Serialize)]
pub struct MessageResponse {
    pub id: Uuid,
    pub channel_id: Uuid,
    pub author_id: Uuid,
    pub author_username: String,
    pub ciphertext: Vec<u8>,
    pub nonce: Vec<u8>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}
