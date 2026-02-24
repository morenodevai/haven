use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use serde::Deserialize;
use tracing::{error, warn};
use uuid::Uuid;

use haven_types::api::{MessageResponse, ReactionGroup, SendMessageRequest};
use haven_types::events::GatewayEvent;

use crate::auth::AppStateInner;
use crate::middleware::Claims;

#[derive(Debug, Deserialize)]
pub struct MessageQuery {
    #[serde(default = "default_limit")]
    pub limit: u32,
    /// #11: Cursor-based pagination — pass the `created_at` timestamp of the
    /// oldest message from the previous page to fetch older messages.
    pub before: Option<String>,
}

fn default_limit() -> u32 {
    50
}

/// #8: Channel authorization model — all authenticated users can access all channels.
/// This is by design for the current MVP: Haven is a small private server where all
/// registered users are trusted. Per-channel ACLs are a future feature.
pub async fn send_message(
    State(state): State<Arc<AppStateInner>>,
    Path(channel_id): Path<Uuid>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<SendMessageRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let message_id = Uuid::new_v4();

    let ciphertext_bytes = B64.decode(&req.ciphertext).map_err(|_| StatusCode::BAD_REQUEST)?;
    let nonce_bytes = B64.decode(&req.nonce).map_err(|_| StatusCode::BAD_REQUEST)?;

    // Run blocking DB insert off the async runtime
    let db = state.clone();
    let cid = channel_id.to_string();
    let mid = message_id.to_string();
    let aid = claims.sub.to_string();
    tokio::task::spawn_blocking(move || {
        db.db.insert_message(&mid, &cid, &aid, &ciphertext_bytes, &nonce_bytes)
    })
    .await
    .map_err(|e| { error!("spawn_blocking join error: {}", e); StatusCode::INTERNAL_SERVER_ERROR })?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let now = chrono::Utc::now();

    // Broadcast to all WebSocket clients
    state.dispatcher.broadcast(GatewayEvent::MessageCreate {
        id: message_id,
        channel_id,
        author_id: claims.sub,
        author_username: claims.username.clone(),
        ciphertext: req.ciphertext.clone(),
        nonce: req.nonce.clone(),
        timestamp: now,
    });

    Ok((StatusCode::CREATED, Json(MessageResponse {
        id: message_id,
        channel_id,
        author_id: claims.sub,
        author_username: claims.username.clone(),
        ciphertext: req.ciphertext,
        nonce: req.nonce,
        created_at: now,
        reactions: vec![],
    })))
}

pub async fn get_messages(
    State(state): State<Arc<AppStateInner>>,
    Path(channel_id): Path<Uuid>,
    Query(query): Query<MessageQuery>,
    Extension(_claims): Extension<Claims>,
) -> Result<impl IntoResponse, StatusCode> {
    // Run all blocking DB queries off the async runtime
    let db = state.clone();
    let cid = channel_id.to_string();
    let limit = query.limit.min(200);
    let before = query.before;

    let (rows, reaction_rows) = tokio::task::spawn_blocking(move || {
        // #11: Cursor-based pagination via `before` parameter
        let rows = db
            .db
            .get_messages(&cid, limit, before.as_deref())
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        let message_ids: Vec<String> = rows.iter().map(|r| r.id.clone()).collect();
        let reaction_rows = db
            .db
            .get_reactions_for_messages(&message_ids)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        Ok::<_, StatusCode>((rows, reaction_rows))
    })
    .await
    .map_err(|e| { error!("spawn_blocking join error: {}", e); StatusCode::INTERNAL_SERVER_ERROR })??;

    // Group reactions by message_id -> emoji -> user_ids (cheap in-memory work, fine on async thread)
    let mut reaction_map: HashMap<String, HashMap<String, Vec<Uuid>>> = HashMap::new();
    for r in &reaction_rows {
        let emoji_map = reaction_map.entry(r.message_id.clone()).or_default();
        let user_ids = emoji_map.entry(r.emoji.clone()).or_default();
        if let Ok(uid) = r.user_id.parse::<Uuid>() {
            user_ids.push(uid);
        }
    }

    let messages: Vec<MessageResponse> = rows
        .into_iter()
        .map(|row| {
            let author_username = row.author_username.clone();

            let reactions = reaction_map
                .get(&row.id)
                .map(|emoji_map| {
                    emoji_map
                        .iter()
                        .map(|(emoji, user_ids)| ReactionGroup {
                            emoji: emoji.clone(),
                            count: user_ids.len(),
                            user_ids: user_ids.clone(),
                        })
                        .collect()
                })
                .unwrap_or_default();

            MessageResponse {
                id: row.id.parse().unwrap_or_else(|e| {
                    warn!("Corrupt message id '{}': {}", row.id, e);
                    Uuid::default()
                }),
                channel_id: row.channel_id.parse().unwrap_or_else(|e| {
                    warn!("Corrupt channel_id '{}' on message '{}': {}", row.channel_id, row.id, e);
                    Uuid::default()
                }),
                author_id: row.author_id.parse().unwrap_or_else(|e| {
                    warn!("Corrupt author_id '{}' on message '{}': {}", row.author_id, row.id, e);
                    Uuid::default()
                }),
                author_username,
                ciphertext: B64.encode(&row.ciphertext),
                nonce: B64.encode(&row.nonce),
                created_at: row
                    .created_at
                    .parse::<chrono::DateTime<chrono::Utc>>()
                    .or_else(|_| {
                        // SQLite stores timestamps as "YYYY-MM-DD HH:MM:SS" without timezone.
                        // Parse as naive UTC and convert.
                        chrono::NaiveDateTime::parse_from_str(&row.created_at, "%Y-%m-%d %H:%M:%S")
                            .map(|ndt| ndt.and_utc())
                    })
                    .unwrap_or_else(|e| {
                        warn!("Corrupt created_at '{}' on message '{}': {}", row.created_at, row.id, e);
                        chrono::DateTime::default()
                    }),
                reactions,
            }
        })
        .collect();

    Ok(Json(messages))
}
