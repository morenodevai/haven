use std::sync::Arc;

use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
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
    let _ = channel_id; // validated by path extraction

    let reaction_id = Uuid::new_v4();

    let (added, _id) = state
        .db
        .toggle_reaction(
            &reaction_id.to_string(),
            &message_id.to_string(),
            &claims.sub.to_string(),
            &req.emoji,
        )
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
