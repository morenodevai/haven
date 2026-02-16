use std::sync::Arc;

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Deserialize;
use uuid::Uuid;

use haven_types::api::{MessageResponse, SendMessageRequest};
use haven_types::events::GatewayEvent;

use crate::auth::AppStateInner;
use crate::middleware::Claims;

#[derive(Debug, Deserialize)]
pub struct MessageQuery {
    #[serde(default = "default_limit")]
    pub limit: u32,
}

fn default_limit() -> u32 {
    50
}

pub async fn send_message(
    State(state): State<Arc<AppStateInner>>,
    Path(channel_id): Path<Uuid>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<SendMessageRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let message_id = Uuid::new_v4();

    state
        .db
        .insert_message(
            &message_id.to_string(),
            &channel_id.to_string(),
            &claims.sub.to_string(),
            &req.ciphertext,
            &req.nonce,
        )
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
    })))
}

pub async fn get_messages(
    State(state): State<Arc<AppStateInner>>,
    Path(channel_id): Path<Uuid>,
    Query(query): Query<MessageQuery>,
    Extension(_claims): Extension<Claims>,
) -> Result<impl IntoResponse, StatusCode> {
    let rows = state
        .db
        .get_messages(&channel_id.to_string(), query.limit)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let messages: Vec<MessageResponse> = rows
        .into_iter()
        .map(|row| {
            let author_username = state
                .db
                .get_username_by_id(&row.author_id)
                .unwrap_or_else(|_| "unknown".into());

            MessageResponse {
                id: row.id.parse().unwrap_or_default(),
                channel_id: row.channel_id.parse().unwrap_or_default(),
                author_id: row.author_id.parse().unwrap_or_default(),
                author_username,
                ciphertext: row.ciphertext,
                nonce: row.nonce,
                created_at: row
                    .created_at
                    .parse::<chrono::DateTime<chrono::Utc>>()
                    .unwrap_or_default(),
            }
        })
        .collect();

    Ok(Json(messages))
}
