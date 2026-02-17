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

    // Step 3: Register as online and register per-user channel
    dispatcher.user_online(user_id, username.clone()).await;
    let (conn_id, mut user_rx) = dispatcher.register_user_channel(user_id).await;

    // Step 4: Subscribe to broadcasts and relay to this client
    let mut broadcast_rx = dispatcher.subscribe();
    let dispatcher_clone = dispatcher.clone();

    // Spawn task to forward broadcasts + targeted messages -> client
    let mut send_task = tokio::spawn(async move {
        loop {
            let event = tokio::select! {
                result = broadcast_rx.recv() => {
                    match result {
                        Ok(event) => event,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(_) => break,
                    }
                }
                result = user_rx.recv() => {
                    match result {
                        Some(event) => event,
                        None => break,
                    }
                }
            };

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
                    match serde_json::from_str::<GatewayCommand>(&text) {
                        Ok(cmd) => {
                            handle_command(&dispatcher_clone, user_id, &username_recv, cmd).await;
                        }
                        Err(e) => {
                            warn!("{} ({}) bad command: {} — raw: {}", username_recv, user_id, e, &text[..text.len().min(200)]);
                        }
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

    dispatcher.user_offline(user_id, conn_id).await;
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

        GatewayCommand::VoiceJoin { channel_id } => {
            info!("{} ({}) joining voice channel {}", username, user_id, channel_id);
            let session_id = Uuid::new_v4().to_string();
            let existing = dispatcher
                .voice_join(channel_id, user_id, username.to_string(), session_id.clone())
                .await;

            // Send existing participants to the new joiner so they see who's already here
            for p in &existing {
                dispatcher.send_to_user(user_id, GatewayEvent::VoiceStateUpdate {
                    channel_id,
                    user_id: p.user_id,
                    username: p.username.clone(),
                    session_id: Some(p.session_id.clone()),
                    self_mute: p.self_mute,
                    self_deaf: p.self_deaf,
                }).await;
            }

            // Broadcast the new joiner to everyone (including themselves)
            dispatcher.broadcast(GatewayEvent::VoiceStateUpdate {
                channel_id,
                user_id,
                username: username.to_string(),
                session_id: Some(session_id),
                self_mute: false,
                self_deaf: false,
            });
        }

        GatewayCommand::VoiceLeave => {
            info!("{} ({}) leaving voice", username, user_id);
            if let Some(channel_id) = dispatcher.voice_leave(user_id).await {
                dispatcher.broadcast(GatewayEvent::VoiceStateUpdate {
                    channel_id,
                    user_id,
                    username: username.to_string(),
                    session_id: None,
                    self_mute: false,
                    self_deaf: false,
                });
            }
        }

        GatewayCommand::VoiceStateSet { self_mute, self_deaf } => {
            if let Some((channel_id, participant)) = dispatcher
                .voice_update_state(user_id, self_mute, self_deaf)
                .await
            {
                dispatcher.broadcast(GatewayEvent::VoiceStateUpdate {
                    channel_id,
                    user_id,
                    username: username.to_string(),
                    session_id: Some(participant.session_id),
                    self_mute: participant.self_mute,
                    self_deaf: participant.self_deaf,
                });
            }
        }

        GatewayCommand::VoiceSignalSend { target_user_id, signal } => {
            info!("{} ({}) -> voice signal to {}", username, user_id, target_user_id);
            dispatcher
                .send_to_user(
                    target_user_id,
                    GatewayEvent::VoiceSignal {
                        from_user_id: user_id,
                        signal,
                    },
                )
                .await;
        }

        GatewayCommand::VoiceData { data } => {
            info!("{} ({}) sending voice data ({} bytes)", username, user_id, data.len());
            dispatcher.relay_voice_data(user_id, data).await;
        }
    }
}
