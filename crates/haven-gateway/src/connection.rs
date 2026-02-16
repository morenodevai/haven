use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use jsonwebtoken::{DecodingKey, Validation, decode};
use tracing::{info, warn};
use uuid::Uuid;

use haven_types::events::{GatewayCommand, GatewayEvent};

use crate::dispatcher::Dispatcher;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct Claims {
    sub: Uuid,
    username: String,
    exp: usize,
}

/// Handle a single WebSocket connection.
pub async fn handle_connection(socket: WebSocket, dispatcher: Dispatcher, jwt_secret: String) {
    let (mut sender, mut receiver) = socket.split();

    // Step 1: Wait for Identify command with JWT
    let (user_id, username) = match wait_for_identify(&mut receiver, &jwt_secret).await {
        Some(id) => id,
        None => {
            warn!("WebSocket client failed to identify, closing");
            return;
        }
    };

    info!("{} ({}) connected to gateway", username, user_id);

    // Step 2: Send Ready event
    let ready = GatewayEvent::Ready {
        user_id,
        username: username.clone(),
    };
    if sender
        .send(Message::Text(serde_json::to_string(&ready).unwrap().into()))
        .await
        .is_err()
    {
        return;
    }

    // Step 3: Register as online
    dispatcher.user_online(user_id, username.clone()).await;

    // Step 4: Subscribe to broadcasts and relay to this client
    let mut broadcast_rx = dispatcher.subscribe();
    let dispatcher_clone = dispatcher.clone();

    // Spawn task to forward broadcasts -> client
    let mut send_task = tokio::spawn(async move {
        while let Ok(event) = broadcast_rx.recv().await {
            let text = serde_json::to_string(&event).unwrap();
            if sender.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    });

    // Read commands from client
    let username_recv = username.clone();
    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = receiver.next().await {
            match msg {
                Message::Text(text) => {
                    if let Ok(cmd) = serde_json::from_str::<GatewayCommand>(&text) {
                        handle_command(&dispatcher_clone, user_id, &username_recv, cmd).await;
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    });

    // Wait for either task to finish
    tokio::select! {
        _ = &mut send_task => recv_task.abort(),
        _ = &mut recv_task => send_task.abort(),
    }

    dispatcher.user_offline(user_id).await;
    info!("{} ({}) disconnected from gateway", username, user_id);
}

async fn wait_for_identify(
    receiver: &mut futures_util::stream::SplitStream<WebSocket>,
    jwt_secret: &str,
) -> Option<(Uuid, String)> {
    // Give client 10 seconds to identify
    let timeout = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while let Some(Ok(msg)) = receiver.next().await {
            if let Message::Text(text) = msg {
                if let Ok(GatewayCommand::Identify { token }) =
                    serde_json::from_str::<GatewayCommand>(&text)
                {
                    let token_data = decode::<Claims>(
                        &token,
                        &DecodingKey::from_secret(jwt_secret.as_bytes()),
                        &Validation::default(),
                    )
                    .ok()?;

                    return Some((token_data.claims.sub, token_data.claims.username));
                }
            }
        }
        None
    });

    timeout.await.ok().flatten()
}

async fn handle_command(dispatcher: &Dispatcher, user_id: Uuid, username: &str, cmd: GatewayCommand) {
    match cmd {
        GatewayCommand::Identify { .. } => {} // Already handled
        GatewayCommand::StartTyping { channel_id } => {
            dispatcher.broadcast(GatewayEvent::TypingStart {
                channel_id,
                user_id,
                username: username.to_string(),
            });
        }
    }
}
