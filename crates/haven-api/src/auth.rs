use std::sync::Arc;

use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier, password_hash::{SaltString, rand_core::OsRng}};
use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use jsonwebtoken::{EncodingKey, Header, encode};
use uuid::Uuid;

use haven_db::Database;
use haven_types::api::{LoginRequest, LoginResponse, RegisterRequest, RegisterResponse};

use crate::middleware::Claims;

pub type AppState = Arc<AppStateInner>;

pub struct AppStateInner {
    pub db: Database,
    pub jwt_secret: String,
}

pub async fn register(
    State(state): State<AppState>,
    Json(req): Json<RegisterRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    // Validate input
    if req.username.len() < 3 || req.username.len() > 32 {
        return Err(StatusCode::BAD_REQUEST);
    }
    if req.password.len() < 8 {
        return Err(StatusCode::BAD_REQUEST);
    }

    // Check if username is taken
    if state
        .db
        .get_user_by_username(&req.username)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .is_some()
    {
        return Err(StatusCode::CONFLICT);
    }

    // Hash password with Argon2id
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let password_hash = argon2
        .hash_password(req.password.as_bytes(), &salt)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .to_string();

    let user_id = Uuid::new_v4();

    state
        .db
        .create_user(&user_id.to_string(), &req.username, &password_hash)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let token = create_token(&state.jwt_secret, user_id, &req.username)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok((
        StatusCode::CREATED,
        Json(RegisterResponse {
            user_id,
            token,
        }),
    ))
}

pub async fn login(
    State(state): State<AppState>,
    Json(req): Json<LoginRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let user = state
        .db
        .get_user_by_username(&req.username)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::UNAUTHORIZED)?;

    // Verify password
    let parsed_hash =
        PasswordHash::new(&user.password).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Argon2::default()
        .verify_password(req.password.as_bytes(), &parsed_hash)
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    let user_id: Uuid = user.id.parse().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let token = create_token(&state.jwt_secret, user_id, &user.username)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(LoginResponse {
        user_id,
        username: user.username,
        token,
    }))
}

fn create_token(secret: &str, user_id: Uuid, username: &str) -> anyhow::Result<String> {
    let claims = Claims {
        sub: user_id,
        username: username.to_string(),
        exp: (chrono::Utc::now() + chrono::Duration::days(30)).timestamp() as usize,
    };

    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )?;

    Ok(token)
}
