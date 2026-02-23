use std::sync::Arc;

use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use tracing::error;
use uuid::Uuid;

use haven_types::api::ToggleReactionRequest;
use haven_types::events::GatewayEvent;

use crate::auth::AppStateInner;
use crate::middleware::Claims;

pub async fn toggle_reaction(
    State(state): State<Arc<AppStateInner>>,
    Path((channel_id, message_id)): Path<(Uuid, Uuid)>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<ToggleReactionRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    // M2: Validate emoji length — 64 bytes is generous for any real emoji sequence
    if req.emoji.is_empty() || req.emoji.len() > 64 {
        return Err(StatusCode::BAD_REQUEST);
    }

    // #34: Validate that the message actually belongs to the specified channel.
    // Without this, a client could toggle a reaction on a message in a different
    // channel by crafting the request path.
    let db_check = state.clone();
    let mid = message_id.to_string();
    let cid = channel_id.to_string();
    let belongs = tokio::task::spawn_blocking(move || {
        db_check.db.message_belongs_to_channel(&mid, &cid)
    })
    .await
    .map_err(|e| { error!("spawn_blocking join error: {}", e); StatusCode::INTERNAL_SERVER_ERROR })?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    if !belongs {
        return Err(StatusCode::NOT_FOUND);
    }

    let reaction_id = Uuid::new_v4();

    // Run blocking DB call off the async runtime
    let db = state.clone();
    let rid = reaction_id.to_string();
    let mid = message_id.to_string();
    let uid = claims.sub.to_string();
    let emoji = req.emoji.clone();
    let (added, _id) = tokio::task::spawn_blocking(move || {
        db.db.toggle_reaction(&rid, &mid, &uid, &emoji)
    })
    .await
    .map_err(|e| { error!("spawn_blocking join error: {}", e); StatusCode::INTERNAL_SERVER_ERROR })?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    if added {
        state.dispatcher.broadcast(GatewayEvent::ReactionAdd {
            message_id,
            user_id: claims.sub,
            username: claims.username.clone(),
            emoji: req.emoji,
        });
    } else {
        state.dispatcher.broadcast(GatewayEvent::ReactionRemove {
            message_id,
            user_id: claims.sub,
            emoji: req.emoji,
        });
    }

    Ok(Json(serde_json::json!({ "added": added })))
}
