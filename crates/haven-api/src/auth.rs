use std::collections::HashMap;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier, password_hash::{SaltString, rand_core::OsRng}};
use axum::{Json, extract::{ConnectInfo, State}, http::StatusCode, response::IntoResponse};
use jsonwebtoken::{EncodingKey, Header, encode};
use std::net::SocketAddr;
use uuid::Uuid;

use haven_db::Database;
use haven_gateway::dispatcher::Dispatcher;
use haven_types::api::{LoginRequest, LoginResponse, RegisterRequest, RegisterResponse};

use crate::middleware::Claims;

/// Maximum password length in bytes. Prevents DoS via expensive Argon2 hashing
/// on extremely long inputs.
const MAX_PASSWORD_LEN: usize = 128;

/// Rate limiter: max attempts per IP within the sliding window.
const RATE_LIMIT_MAX_ATTEMPTS: u32 = 10;
/// Sliding window duration in seconds.
const RATE_LIMIT_WINDOW_SECS: u64 = 60;

pub type AppState = Arc<AppStateInner>;

pub struct AppStateInner {
    pub db: Database,
    pub jwt_secret: String,
    pub dispatcher: Dispatcher,
    pub auth_rate_limiter: AuthRateLimiter,
    /// #14: Configurable uploads directory (absolute path).
    pub uploads_dir: PathBuf,
}

/// Simple sliding-window rate limiter keyed by IP address.
/// No external dependencies required — uses only std.
#[derive(Clone)]
pub struct AuthRateLimiter {
    state: Arc<Mutex<HashMap<IpAddr, Vec<Instant>>>>,
    /// #13: Counter for periodic full sweep to prevent memory leak from stale IPs.
    call_count: Arc<AtomicU64>,
}

impl AuthRateLimiter {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(HashMap::new())),
            call_count: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Returns `true` if the request is allowed, `false` if rate-limited.
    /// Prunes expired entries from the window on each call.
    pub fn check(&self, ip: IpAddr) -> bool {
        let mut map = self.state.lock().unwrap();
        let now = Instant::now();
        let window = std::time::Duration::from_secs(RATE_LIMIT_WINDOW_SECS);

        // #13: Every 100th call, do a full sweep of all IPs to prune stale entries.
        // This prevents unbounded memory growth from unique IPs that never return.
        let count = self.call_count.fetch_add(1, Ordering::Relaxed);
        if count % 100 == 0 {
            map.retain(|_, timestamps| {
                timestamps.retain(|t| now.duration_since(*t) < window);
                !timestamps.is_empty()
            });
        }

        let timestamps = map.entry(ip).or_default();

        // Prune entries older than the window
        timestamps.retain(|t| now.duration_since(*t) < window);

        if timestamps.len() as u32 >= RATE_LIMIT_MAX_ATTEMPTS {
            return false;
        }

        timestamps.push(now);
        true
    }
}

impl Default for AuthRateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

/// #30: Only allow alphanumeric characters, underscores, and hyphens in usernames.
fn is_valid_username(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

pub async fn register(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(req): Json<RegisterRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    // #12: Rate limit registration by IP
    if !state.auth_rate_limiter.check(addr.ip()) {
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }

    // Validate input
    if req.username.len() < 3 || req.username.len() > 32 {
        return Err(StatusCode::BAD_REQUEST);
    }
    // #30: Username content validation
    if !is_valid_username(&req.username) {
        return Err(StatusCode::BAD_REQUEST);
    }
    if req.password.len() < 8 || req.password.len() > MAX_PASSWORD_LEN {
        return Err(StatusCode::BAD_REQUEST);
    }

    // All blocking work (DB queries + Argon2 hashing) off the async runtime
    let state_clone = state.clone();
    let username = req.username.clone();
    let password = req.password.clone();

    let (user_id, token) = tokio::task::spawn_blocking(move || {
        // Check if username is taken
        if state_clone
            .db
            .get_user_by_username(&username)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .is_some()
        {
            return Err(StatusCode::CONFLICT);
        }

        // Hash password with Argon2id
        let salt = SaltString::generate(&mut OsRng);
        let argon2 = Argon2::default();
        let password_hash = argon2
            .hash_password(password.as_bytes(), &salt)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .to_string();

        let user_id = Uuid::new_v4();

        state_clone
            .db
            .create_user(&user_id.to_string(), &username, &password_hash)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        let token = create_token(&state_clone.jwt_secret, user_id, &username)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        Ok((user_id, token))
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)??;

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
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(req): Json<LoginRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    // #12: Rate limit login by IP
    if !state.auth_rate_limiter.check(addr.ip()) {
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }

    // Reject excessively long passwords (DoS prevention)
    if req.password.len() > MAX_PASSWORD_LEN {
        return Err(StatusCode::BAD_REQUEST);
    }

    // All blocking work (DB lookup + Argon2 verification) off the async runtime
    let state_clone = state.clone();
    let username = req.username.clone();
    let password = req.password.clone();

    let (user_id, response_username, token) = tokio::task::spawn_blocking(move || {
        let user = state_clone
            .db
            .get_user_by_username(&username)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .ok_or(StatusCode::UNAUTHORIZED)?;

        // Verify password
        let parsed_hash =
            PasswordHash::new(&user.password).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        Argon2::default()
            .verify_password(password.as_bytes(), &parsed_hash)
            .map_err(|_| StatusCode::UNAUTHORIZED)?;

        let user_id: Uuid = user.id.parse().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        let token = create_token(&state_clone.jwt_secret, user_id, &user.username)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        Ok::<_, StatusCode>((user_id, user.username, token))
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)??;

    Ok(Json(LoginResponse {
        user_id,
        username: response_username,
        token,
    }))
}

/// Refresh a valid JWT — returns a new token with a fresh expiry.
/// The caller must be authenticated (require_auth middleware).
pub async fn refresh_token(
    State(state): State<AppState>,
    claims: axum::Extension<Claims>,
) -> Result<impl IntoResponse, StatusCode> {
    let token = create_token(&state.jwt_secret, claims.sub, &claims.username)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(serde_json::json!({ "token": token })))
}

fn create_token(secret: &str, user_id: Uuid, username: &str) -> anyhow::Result<String> {
    let claims = Claims {
        sub: user_id,
        username: username.to_string(),
        // #7: JWT expiry reduced from 7 days to 24 hours
        exp: (chrono::Utc::now() + chrono::Duration::hours(24)).timestamp() as usize,
    };

    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )?;

    Ok(token)
}
