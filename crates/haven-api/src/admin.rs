use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    Json,
    extract::{Path, Query, State, WebSocketUpgrade, ws::{Message, WebSocket}},
    http::{StatusCode, header, Request},
    middleware::Next,
    response::{IntoResponse, Response},
};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};
use uuid::Uuid;

use crate::middleware::JwtSecret;
use haven_gateway::dispatcher::Dispatcher;

// ── Types ────────────────────────────────────────────────────────────────

/// JWT claims for admin sessions. Distinct from user Claims -- admin tokens
/// carry `admin: true` and no user identity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminClaims {
    pub admin: bool,
    pub exp: usize,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdminLoginRequest {
    pub secret: String,
}

#[derive(Debug, Serialize)]
pub struct AdminLoginResponse {
    pub token: String,
}

/// State shared by all admin handlers.
#[derive(Clone)]
pub struct AdminState {
    pub admin_secret: String,
    pub jwt_secret: String,
    pub db: Arc<haven_db::Database>,
    pub dispatcher: Dispatcher,
    pub http_client: reqwest::Client,
    pub file_server_internal_url: Option<String>,
    pub start_time: Instant,
}

#[derive(Debug, Deserialize)]
pub struct MessageQuery {
    pub limit: Option<u32>,
    pub channel_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateChannelRequest {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct AdminWsQuery {
    pub token: Option<String>,
}

// ── Auth ─────────────────────────────────────────────────────────────────

/// POST /admin/login
/// Validates the provided secret against HAVEN_ADMIN_SECRET env var.
/// Returns a JWT with `admin: true` claim, 8-hour expiry.
pub async fn admin_login(
    State(state): State<AdminState>,
    Json(req): Json<AdminLoginRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    if req.secret != state.admin_secret {
        return Err(StatusCode::UNAUTHORIZED);
    }

    let claims = AdminClaims {
        admin: true,
        exp: (chrono::Utc::now() + chrono::Duration::hours(8)).timestamp() as usize,
    };

    let token = jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(state.jwt_secret.as_bytes()),
    )
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    info!("Admin login successful");
    Ok(Json(AdminLoginResponse { token }))
}

/// Middleware: validates JWT has `admin: true` claim.
pub async fn require_admin(req: Request<axum::body::Body>, next: Next) -> Result<Response, StatusCode> {
    let auth_header = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let token = auth_header
        .strip_prefix("Bearer ")
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let secret = req
        .extensions()
        .get::<JwtSecret>()
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?
        .clone();

    let token_data = jsonwebtoken::decode::<AdminClaims>(
        token,
        &jsonwebtoken::DecodingKey::from_secret(secret.0.as_bytes()),
        &jsonwebtoken::Validation::default(),
    )
    .map_err(|_| StatusCode::UNAUTHORIZED)?;

    if !token_data.claims.admin {
        return Err(StatusCode::FORBIDDEN);
    }

    Ok(next.run(req).await)
}

// ── Stats ────────────────────────────────────────────────────────────────

/// GET /admin/stats
pub async fn get_stats(State(state): State<AdminState>) -> Result<Json<serde_json::Value>, StatusCode> {
    let stats = build_stats(&state).await?;
    Ok(Json(stats))
}

async fn build_stats(state: &AdminState) -> Result<serde_json::Value, StatusCode> {
    let online = state.dispatcher.online_users().await;
    let uptime_secs = state.start_time.elapsed().as_secs();

    let db = state.db.clone();
    let counts = tokio::task::spawn_blocking(move || {
        db.with_conn(|conn| {
            conn.query_row(
                "SELECT
                    (SELECT COUNT(*) FROM users),
                    (SELECT COUNT(*) FROM messages),
                    (SELECT COUNT(*) FROM channels),
                    (SELECT COUNT(*) FROM pending_offers),
                    (SELECT COUNT(*) FROM pending_folder_offers)",
                [],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?,
                         r.get::<_, i64>(3)?, r.get::<_, i64>(4)?)),
            ).map_err(anyhow::Error::from)
        })
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(serde_json::json!({
        "uptime_secs": uptime_secs,
        "online_count": online.len(),
        "total_users": counts.0,
        "total_messages": counts.1,
        "total_channels": counts.2,
        "pending_file_offers": counts.3,
        "pending_folder_offers": counts.4,
    }))
}

// ── Users ────────────────────────────────────────────────────────────────

/// GET /admin/users
pub async fn list_users(State(state): State<AdminState>) -> Result<Json<serde_json::Value>, StatusCode> {
    let online = state.dispatcher.online_users().await;
    let online_ids: std::collections::HashSet<String> = online.iter().map(|(id, _)| id.to_string()).collect();

    let db = state.db.clone();
    let users = tokio::task::spawn_blocking(move || db.list_all_users())
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let users_json: Vec<serde_json::Value> = users
        .into_iter()
        .map(|(id, username, created_at)| {
            serde_json::json!({
                "id": id,
                "username": username,
                "created_at": created_at,
                "online": online_ids.contains(&id),
            })
        })
        .collect();

    Ok(Json(serde_json::json!({ "users": users_json })))
}

/// DELETE /admin/users/{id}
pub async fn delete_user(
    State(state): State<AdminState>,
    Path(user_id): Path<Uuid>,
) -> Result<StatusCode, StatusCode> {
    let uid = user_id.to_string();

    // Force-disconnect if online
    state.dispatcher.force_disconnect(user_id).await;

    let db = state.db.clone();
    tokio::task::spawn_blocking(move || db.delete_user(&uid))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    info!("Admin deleted user {}", user_id);
    Ok(StatusCode::NO_CONTENT)
}

/// POST /admin/kick/{user_id}
pub async fn kick_user(
    State(state): State<AdminState>,
    Path(user_id): Path<Uuid>,
) -> Result<StatusCode, StatusCode> {
    state.dispatcher.force_disconnect(user_id).await;
    info!("Admin kicked user {}", user_id);
    Ok(StatusCode::NO_CONTENT)
}

// ── Voice ────────────────────────────────────────────────────────────────

/// GET /admin/voice
pub async fn get_voice_state(State(state): State<AdminState>) -> Result<Json<serde_json::Value>, StatusCode> {
    let voice = state.dispatcher.voice_states().await;

    let channels_json: Vec<serde_json::Value> = voice
        .into_iter()
        .map(|(channel_id, participants)| {
            let ps: Vec<serde_json::Value> = participants
                .into_iter()
                .map(|p| {
                    serde_json::json!({
                        "user_id": p.user_id,
                        "username": p.username,
                        "session_id": p.session_id,
                        "self_mute": p.self_mute,
                        "self_deaf": p.self_deaf,
                    })
                })
                .collect();
            serde_json::json!({
                "channel_id": channel_id,
                "participants": ps,
            })
        })
        .collect();

    Ok(Json(serde_json::json!({ "channels": channels_json })))
}

// ── Messages ─────────────────────────────────────────────────────────────

/// GET /admin/messages?limit=50&channel_id=...
pub async fn list_messages(
    State(state): State<AdminState>,
    Query(params): Query<MessageQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let limit = params.limit.unwrap_or(50).min(200);
    let channel_id = params.channel_id.clone();

    let db = state.db.clone();
    let messages = tokio::task::spawn_blocking(move || {
        db.list_recent_messages(limit, channel_id.as_deref())
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let msgs_json: Vec<serde_json::Value> = messages
        .into_iter()
        .map(|m| {
            serde_json::json!({
                "id": m.id,
                "channel_id": m.channel_id,
                "author_id": m.author_id,
                "author_username": m.author_username,
                "byte_length": m.ciphertext.len(),
                "created_at": m.created_at,
            })
        })
        .collect();

    Ok(Json(serde_json::json!({ "messages": msgs_json })))
}

// ── Offers ───────────────────────────────────────────────────────────────

/// GET /admin/offers
pub async fn list_offers(State(state): State<AdminState>) -> Result<Json<serde_json::Value>, StatusCode> {
    let db = state.db.clone();
    let (file_offers, folder_offers) = tokio::task::spawn_blocking(move || {
        let fo = db.list_all_pending_offers()?;
        let ffo = db.list_all_pending_folder_offers()?;
        Ok::<_, anyhow::Error>((fo, ffo))
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let file_json: Vec<serde_json::Value> = file_offers
        .into_iter()
        .map(|o| {
            serde_json::json!({
                "transfer_id": o.transfer_id,
                "from_user_id": o.from_user_id,
                "to_user_id": o.to_user_id,
                "filename": o.filename,
                "file_size": o.file_size,
                "folder_id": o.folder_id,
                "status": o.status,
            })
        })
        .collect();

    let folder_json: Vec<serde_json::Value> = folder_offers
        .into_iter()
        .map(|f| {
            serde_json::json!({
                "folder_id": f.folder_id,
                "from_user_id": f.from_user_id,
                "to_user_id": f.to_user_id,
                "folder_name": f.folder_name,
                "total_size": f.total_size,
                "file_count": f.file_count,
                "status": f.status,
            })
        })
        .collect();

    Ok(Json(serde_json::json!({
        "file_offers": file_json,
        "folder_offers": folder_json,
    })))
}

/// DELETE /admin/offers/{transfer_id}
pub async fn delete_offer(
    State(state): State<AdminState>,
    Path(transfer_id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || db.delete_pending_offer(&transfer_id))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(StatusCode::NO_CONTENT)
}

// ── Transfers (proxied to file server) ───────────────────────────────────

/// GET /admin/transfers
pub async fn list_transfers(State(state): State<AdminState>) -> Result<Json<serde_json::Value>, StatusCode> {
    let base = state
        .file_server_internal_url
        .as_deref()
        .unwrap_or("http://127.0.0.1:3211");
    let url = format!("{}/transfers/all", base);

    let resp = state
        .http_client
        .get(&url)
        .send()
        .await
        .map_err(|e| {
            warn!("Admin transfers proxy failed: {}", e);
            StatusCode::BAD_GATEWAY
        })?;

    let bytes = resp.bytes().await.map_err(|e| {
        warn!("Admin transfers proxy read failed: {}", e);
        StatusCode::BAD_GATEWAY
    })?;

    let body: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| {
        warn!("Admin transfers proxy parse failed: {}", e);
        StatusCode::BAD_GATEWAY
    })?;

    Ok(Json(body))
}

/// DELETE /admin/transfers/{id}
pub async fn delete_transfer(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, StatusCode> {
    let base = state
        .file_server_internal_url
        .as_deref()
        .unwrap_or("http://127.0.0.1:3211");
    let url = format!("{}/admin/transfers/{}", base, id);

    let resp = state
        .http_client
        .delete(&url)
        .send()
        .await
        .map_err(|e| {
            warn!("Admin delete transfer proxy failed: {}", e);
            StatusCode::BAD_GATEWAY
        })?;

    Ok(StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY))
}

// ── Config ───────────────────────────────────────────────────────────────

/// GET /admin/config
pub async fn get_config() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "haven_port": std::env::var("HAVEN_PORT").unwrap_or_default(),
        "haven_host": std::env::var("HAVEN_HOST").unwrap_or_default(),
        "haven_db_path": std::env::var("HAVEN_DB_PATH").unwrap_or_default(),
        "file_server_url": std::env::var("HAVEN_FILE_SERVER_URL").unwrap_or_default(),
        "file_server_internal_url": std::env::var("HAVEN_FILE_SERVER_INTERNAL_URL").unwrap_or_default(),
        "haven_jwt_secret": "***REDACTED***",
        "haven_admin_secret": "***REDACTED***",
        "haven_turn_user": std::env::var("HAVEN_TURN_USER").unwrap_or_default(),
        "haven_turn_pass": "***REDACTED***",
        "haven_public_ip": std::env::var("HAVEN_PUBLIC_IP").unwrap_or_default(),
    }))
}

// ── Channels ─────────────────────────────────────────────────────────────

/// GET /admin/channels
pub async fn list_channels(State(state): State<AdminState>) -> Result<Json<serde_json::Value>, StatusCode> {
    let db = state.db.clone();
    let channels = tokio::task::spawn_blocking(move || db.list_channels_with_counts())
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let json: Vec<serde_json::Value> = channels
        .into_iter()
        .map(|(id, name, count)| {
            serde_json::json!({
                "id": id,
                "name": name,
                "message_count": count,
            })
        })
        .collect();

    Ok(Json(serde_json::json!({ "channels": json })))
}

/// POST /admin/channels
pub async fn create_channel(
    State(state): State<AdminState>,
    Json(req): Json<CreateChannelRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let id = Uuid::new_v4().to_string();
    let name = req.name.clone();

    let db = state.db.clone();
    let channel_id = id.clone();
    let channel_name = name.clone();
    tokio::task::spawn_blocking(move || db.create_channel(&channel_id, &channel_name))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    info!("Admin created channel '{}' ({})", name, id);
    Ok(Json(serde_json::json!({ "id": id, "name": name })))
}

/// DELETE /admin/channels/{id}
pub async fn delete_channel(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, StatusCode> {
    let channel_id = id.to_string();

    let db = state.db.clone();
    tokio::task::spawn_blocking(move || db.delete_channel(&channel_id))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    info!("Admin deleted channel {}", id);
    Ok(StatusCode::NO_CONTENT)
}

// ── Admin WebSocket ──────────────────────────────────────────────────────

/// GET /admin/ws (WebSocket upgrade)
/// Authenticates via ?token= query param (admin JWT).
pub async fn admin_ws(
    State(state): State<AdminState>,
    Query(query): Query<AdminWsQuery>,
    ws: WebSocketUpgrade,
) -> Result<impl IntoResponse, StatusCode> {
    let token = query.token.ok_or(StatusCode::UNAUTHORIZED)?;

    let token_data = jsonwebtoken::decode::<AdminClaims>(
        &token,
        &jsonwebtoken::DecodingKey::from_secret(state.jwt_secret.as_bytes()),
        &jsonwebtoken::Validation::default(),
    )
    .map_err(|_| StatusCode::UNAUTHORIZED)?;

    if !token_data.claims.admin {
        return Err(StatusCode::FORBIDDEN);
    }

    Ok(ws.on_upgrade(move |socket| admin_ws_handler(socket, state)))
}

async fn admin_ws_handler(socket: WebSocket, state: AdminState) {
    let (mut tx, mut rx) = socket.split();
    let mut broadcast_rx = state.dispatcher.subscribe();
    let mut stats_interval = tokio::time::interval(Duration::from_secs(5));

    loop {
        tokio::select! {
            result = broadcast_rx.recv() => {
                match result {
                    Ok(msg) => {
                        if let Some(admin_json) = map_to_admin_event(&msg.json) {
                            if tx.send(Message::Text(admin_json.into())).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!("Admin WS lagged by {} messages", n);
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
            _ = stats_interval.tick() => {
                if let Ok(stats) = build_stats(&state).await {
                    let msg = serde_json::json!({
                        "type": "stats_update",
                        "data": stats,
                    });
                    if tx.send(Message::Text(msg.to_string().into())).await.is_err() {
                        break;
                    }
                }
            }
            msg = rx.next() => {
                match msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }
}

/// Map a pre-serialized GatewayEvent JSON to an admin event JSON string.
/// Returns None for events the admin dashboard doesn't care about.
fn map_to_admin_event(json: &str) -> Option<String> {
    let event: serde_json::Value = serde_json::from_str(json).ok()?;
    let event_type = event.get("type")?.as_str()?;
    let data = event.get("data")?;

    match event_type {
        "PresenceUpdate" => {
            let online = data.get("online")?.as_bool()?;
            let admin_type = if online { "user_online" } else { "user_offline" };
            Some(
                serde_json::json!({
                    "type": admin_type,
                    "data": {
                        "user_id": data.get("user_id")?,
                        "username": data.get("username")?,
                    }
                })
                .to_string(),
            )
        }
        "VoiceStateUpdate" => Some(
            serde_json::json!({
                "type": "voice_update",
                "data": data
            })
            .to_string(),
        ),
        "MessageCreate" => {
            let ciphertext = data.get("ciphertext")?.as_str()?;
            Some(
                serde_json::json!({
                    "type": "new_message",
                    "data": {
                        "id": data.get("id")?,
                        "channel_id": data.get("channel_id")?,
                        "author_id": data.get("author_id")?,
                        "author_username": data.get("author_username")?,
                        "timestamp": data.get("timestamp")?,
                        "byte_length": ciphertext.len(),
                    }
                })
                .to_string(),
            )
        }
        "FileOffer" => Some(
            serde_json::json!({
                "type": "file_offer",
                "data": {
                    "transfer_id": data.get("transfer_id")?,
                    "from_user_id": data.get("from_user_id")?,
                    "filename": data.get("filename")?,
                    "size": data.get("size")?,
                }
            })
            .to_string(),
        ),
        _ => None,
    }
}
