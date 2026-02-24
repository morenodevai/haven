use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use tracing::{info, trace, warn};
use uuid::Uuid;

use haven_types::events::{GatewayCommand, GatewayEvent};

use crate::dispatcher::{Dispatcher, UserMessage};

/// Heartbeat interval: server sends a Ping every 15 seconds.
/// If 2 consecutive Pongs are missed (~30s), the connection is dropped.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);

/// #6: Handle a pre-authenticated WebSocket connection.
/// The JWT was already validated at the HTTP upgrade layer (main.rs), so we
/// skip the Identify handshake and go straight to Ready + event loop.
pub async fn handle_connection_authenticated(
    socket: WebSocket,
    dispatcher: Dispatcher,
    user_id: Uuid,
    username: String,
    file_server_url: Option<String>,
) {
    let (mut sender, receiver) = socket.split();

    info!("{} ({}) connected to gateway (pre-authenticated)", username, user_id);

    // Send Ready event
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

    // Shared connection loop
    run_connection_loop(sender, receiver, dispatcher, user_id, username, file_server_url).await;
}

/// Handle a single WebSocket connection (legacy path — uses Identify handshake).
/// Kept for backwards compatibility with older clients.
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

    // Shared connection loop (legacy path has no file server URL)
    run_connection_loop(sender, receiver, dispatcher, user_id, username, None).await;
}

/// Shared connection loop — factored out of both handle_connection and
/// handle_connection_authenticated to avoid code duplication.
async fn run_connection_loop(
    mut sender: futures_util::stream::SplitSink<WebSocket, Message>,
    mut receiver: futures_util::stream::SplitStream<WebSocket>,
    dispatcher: Dispatcher,
    user_id: Uuid,
    username: String,
    file_server_url: Option<String>,
) {
    // Register per-user channel and send existing online users, then go online
    let (conn_id, mut user_rx) = dispatcher.register_user_channel(user_id).await;

    // Send existing online users to this client so they see who's already here
    let existing_users = dispatcher.online_users().await;
    for (uid, uname) in &existing_users {
        let event = GatewayEvent::PresenceUpdate {
            user_id: *uid,
            username: uname.clone(),
            online: true,
        };
        if sender
            .send(Message::Text(serde_json::to_string(&event).unwrap().into()))
            .await
            .is_err()
        {
            return;
        }
    }

    // Now mark ourselves online (broadcasts to everyone else)
    dispatcher.user_online(user_id, username.clone()).await;

    // Subscribe to broadcasts and relay to this client
    let mut broadcast_rx = dispatcher.subscribe();
    let dispatcher_clone = dispatcher.clone();

    // Per-connection channel subscriptions (shared between send and recv tasks).
    let subscribed_channels: Arc<std::sync::RwLock<HashSet<Uuid>>> =
        Arc::new(std::sync::RwLock::new(HashSet::new()));
    let send_subscriptions = subscribed_channels.clone();

    // H6: Shared flag for heartbeat
    let pong_received = Arc::new(AtomicBool::new(true));
    let pong_flag_send = pong_received.clone();
    let pong_flag_recv = pong_received.clone();

    // Spawn task to forward broadcasts + targeted messages -> client, with heartbeat
    let mut send_task = tokio::spawn(async move {
        let mut heartbeat = tokio::time::interval(HEARTBEAT_INTERVAL);
        heartbeat.tick().await;
        let mut missed_heartbeats: u8 = 0;

        loop {
            tokio::select! {
                result = broadcast_rx.recv() => {
                    let msg = match result {
                        Ok(msg) => msg,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            warn!("Broadcast receiver lagged by {} messages", n);
                            continue;
                        }
                        Err(_) => break,
                    };

                    if let Some(channel_id) = msg.channel_id {
                        let subs = send_subscriptions.read()
                            .expect("subscription lock poisoned");
                        if !subs.contains(&channel_id) {
                            continue;
                        }
                    }

                    if sender.send(Message::Text(msg.json.to_string().into())).await.is_err() {
                        break;
                    }
                }
                result = user_rx.recv() => {
                    let msg = match result {
                        Some(msg) => msg,
                        None => break,
                    };

                    match msg {
                        UserMessage::Event(event) => {
                            let text = serde_json::to_string(&event).unwrap();
                            if sender.send(Message::Text(text.into())).await.is_err() {
                                break;
                            }
                        }
                        UserMessage::Binary(data) => {
                            if sender.send(Message::Binary(data)).await.is_err() {
                                break;
                            }
                        }
                    }
                }
                _ = heartbeat.tick() => {
                    if pong_flag_send.swap(false, Ordering::Acquire) {
                        missed_heartbeats = 0;
                    } else {
                        missed_heartbeats += 1;
                        if missed_heartbeats >= 2 {
                            warn!("Heartbeat timeout (missed {} pongs), dropping connection", missed_heartbeats);
                            break;
                        }
                    }
                    if sender.send(Message::Ping(vec![].into())).await.is_err() {
                        break;
                    }
                }
            }
        }
    });

    // Read commands from client
    let username_recv = username.clone();
    let recv_subscriptions = subscribed_channels.clone();
    let file_server_url_recv = file_server_url.clone();
    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = receiver.next().await {
            match msg {
                Message::Text(text) => {
                    match serde_json::from_str::<GatewayCommand>(&text) {
                        Ok(cmd) => {
                            handle_command(
                                &dispatcher_clone,
                                user_id,
                                &username_recv,
                                cmd,
                                &recv_subscriptions,
                                file_server_url_recv.as_deref(),
                            )
                            .await;
                        }
                        Err(e) => {
                            warn!(
                                "{} ({}) bad command: {} -- raw: {}",
                                username_recv,
                                user_id,
                                e,
                                &text[..text.len().min(200)]
                            );
                        }
                    }
                }
                Message::Binary(data) => {
                    handle_binary_message(
                        &dispatcher_clone,
                        user_id,
                        &data,
                    ).await;
                }
                Message::Pong(_) => {
                    pong_flag_recv.store(true, Ordering::Release);
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
    use jsonwebtoken::{DecodingKey, Validation, decode};
    use haven_types::api::Claims;

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

async fn handle_command(
    dispatcher: &Dispatcher,
    user_id: Uuid,
    username: &str,
    cmd: GatewayCommand,
    subscriptions: &Arc<std::sync::RwLock<HashSet<Uuid>>>,
    file_server_url: Option<&str>,
) {
    match cmd {
        GatewayCommand::Identify { .. } => {} // Already handled

        GatewayCommand::Subscribe { channel_ids } => {
            info!(
                "{} ({}) subscribing to {} channels",
                username,
                user_id,
                channel_ids.len()
            );
            {
                let mut subs = subscriptions.write()
                    .expect("subscription lock poisoned");
                *subs = channel_ids.iter().copied().collect();
            }
            dispatcher
                .subscribe_channels(user_id, channel_ids)
                .await;
        }

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

            for p in &existing {
                dispatcher
                    .send_to_user(
                        user_id,
                        GatewayEvent::VoiceStateUpdate {
                            channel_id,
                            user_id: p.user_id,
                            username: p.username.clone(),
                            session_id: Some(p.session_id.clone()),
                            self_mute: p.self_mute,
                            self_deaf: p.self_deaf,
                        },
                    )
                    .await;
            }

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

        GatewayCommand::VoiceStateSet {
            self_mute,
            self_deaf,
        } => {
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

        GatewayCommand::VoiceSignalSend {
            target_user_id,
            signal,
        } => {
            let signal_desc = match &signal {
                haven_types::events::VoiceSignalPayload::Offer { .. } => "Offer",
                haven_types::events::VoiceSignalPayload::Answer { .. } => "Answer",
                haven_types::events::VoiceSignalPayload::IceCandidate { .. } => "IceCandidate",
                haven_types::events::VoiceSignalPayload::TrackInfo { track_type, stream_id } => {
                    info!("{} ({}) -> TrackInfo to {} [type={}, stream={}]",
                        username, user_id, target_user_id, track_type, stream_id);
                    "TrackInfo"
                }
            };
            info!(
                "{} ({}) -> voice {} to {}",
                username, user_id, signal_desc, target_user_id
            );
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

        // H9: Voice audio data logged at trace level, not info
        GatewayCommand::VoiceData { data } => {
            trace!(
                "{} ({}) sending voice data ({} bytes)",
                username,
                user_id,
                data.len()
            );
            dispatcher.relay_voice_data(user_id, data).await;
        }

        GatewayCommand::FileOfferSend {
            target_user_id,
            transfer_id,
            filename,
            size,
            file_sha256,
            chunk_hashes,
        } => {
            info!(
                "{} ({}) -> file offer to {} ({})",
                username, user_id, target_user_id, filename
            );
            dispatcher
                .send_to_user(
                    target_user_id,
                    GatewayEvent::FileOffer {
                        from_user_id: user_id,
                        transfer_id,
                        filename,
                        size,
                        file_sha256,
                        chunk_hashes,
                        // Inject file server URL from server config
                        file_server_url: file_server_url.map(|s| s.to_string()),
                    },
                )
                .await;
        }

        GatewayCommand::FileAcceptSend {
            target_user_id,
            transfer_id,
        } => {
            info!(
                "{} ({}) -> file accept to {}",
                username, user_id, target_user_id
            );
            dispatcher
                .send_to_user(
                    target_user_id,
                    GatewayEvent::FileAccept {
                        from_user_id: user_id,
                        transfer_id,
                    },
                )
                .await;
        }

        GatewayCommand::FileRejectSend {
            target_user_id,
            transfer_id,
        } => {
            info!(
                "{} ({}) -> file reject to {}",
                username, user_id, target_user_id
            );
            dispatcher
                .send_to_user(
                    target_user_id,
                    GatewayEvent::FileReject {
                        from_user_id: user_id,
                        transfer_id,
                    },
                )
                .await;
        }

        GatewayCommand::FileSignalSend {
            target_user_id,
            transfer_id,
            signal,
        } => {
            let sig_type = match &signal {
                haven_types::events::VoiceSignalPayload::Offer { .. } => "Offer",
                haven_types::events::VoiceSignalPayload::Answer { .. } => "Answer",
                haven_types::events::VoiceSignalPayload::IceCandidate { .. } => "IceCandidate",
                haven_types::events::VoiceSignalPayload::TrackInfo { .. } => "TrackInfo",
            };
            info!(
                "{} ({}) -> file {} to {} [transfer={}]",
                username, user_id, sig_type, target_user_id, &transfer_id[..8]
            );
            dispatcher
                .send_to_user(
                    target_user_id,
                    GatewayEvent::FileSignal {
                        from_user_id: user_id,
                        transfer_id,
                        signal,
                    },
                )
                .await;
        }

        GatewayCommand::FileChunkSend {
            target_user_id,
            transfer_id,
            chunk_index,
            data,
        } => {
            trace!(
                "{} ({}) -> file chunk {} to {}",
                username, user_id, chunk_index, target_user_id
            );
            dispatcher
                .send_to_user(
                    target_user_id,
                    GatewayEvent::FileChunk {
                        from_user_id: user_id,
                        transfer_id,
                        chunk_index,
                        data,
                    },
                )
                .await;
        }

        GatewayCommand::FileDoneSend {
            target_user_id,
            transfer_id,
        } => {
            info!(
                "{} ({}) -> file done to {}",
                username, user_id, target_user_id
            );
            dispatcher
                .send_to_user(
                    target_user_id,
                    GatewayEvent::FileDone {
                        from_user_id: user_id,
                        transfer_id,
                    },
                )
                .await;
        }

        GatewayCommand::FileAckSend {
            target_user_id,
            transfer_id,
            ack_chunk_index,
        } => {
            trace!(
                "{} ({}) -> file ack {} to {}",
                username, user_id, ack_chunk_index, target_user_id
            );
            dispatcher
                .send_to_user(
                    target_user_id,
                    GatewayEvent::FileAck {
                        from_user_id: user_id,
                        transfer_id,
                        ack_chunk_index,
                    },
                )
                .await;
        }

        GatewayCommand::FileUploadCompleteSend {
            target_user_id,
            transfer_id,
            file_sha256,
            chunk_hashes,
        } => {
            info!(
                "{} ({}) -> file upload complete to {} [transfer={}]",
                username, user_id, target_user_id, &transfer_id[..transfer_id.len().min(8)]
            );
            // Only include file_server_url if the gateway has one configured.
            let fsu = file_server_url.as_deref().filter(|s| !s.is_empty()).map(|s| s.to_string());
            dispatcher
                .send_to_user(
                    target_user_id,
                    GatewayEvent::FileReady {
                        from_user_id: user_id,
                        transfer_id,
                        file_server_url: fsu,
                        file_sha256,
                        chunk_hashes,
                    },
                )
                .await;
        }

        GatewayCommand::LogSend { level, tag, message } => {
            match level.as_str() {
                "ERROR" => tracing::error!("[{}] {} ({}): {}", level, username, tag, message),
                "WARN"  => tracing::warn!("[{}] {} ({}): {}", level, username, tag, message),
                "INFO"  => tracing::info!("[{}] {} ({}): {}", level, username, tag, message),
                _       => tracing::debug!("[{}] {} ({}): {}", level, username, tag, message),
            }
        }

        GatewayCommand::HtpCancelSend { session_id, reason } => {
            info!(
                "{} ({}) HTP cancel: session={} reason={}",
                username, user_id, session_id, reason
            );
        }

    }
}

/// Handle incoming binary WebSocket frames for the file transfer fast path
/// and voice audio binary fast path.
///
/// Binary protocol (all multi-byte integers are big-endian):
///
///   0x01 FileChunkSend:  [type(1)] [target_uid(16)] [transfer_id(16)] [chunk_idx(4)] [payload...]
///   0x02 FileAckSend:    [type(1)] [target_uid(16)] [transfer_id(16)] [ack_chunk_idx(4)]
///   0x03 FileDoneSend:   [type(1)] [target_uid(16)] [transfer_id(16)]
///   0x04 VoiceAudio:     [type(1)] [encrypted_payload...]
///
/// For 0x01-0x03: The server swaps `target_user_id` for `sender_user_id`
/// and forwards the frame to the target — zero-copy relay for the encrypted payload.
///
/// For 0x04: The server prepends the sender's UUID and relays to all other
/// voice channel participants as binary frames.
async fn handle_binary_message(
    dispatcher: &Dispatcher,
    sender_user_id: Uuid,
    data: &[u8],
) {
    if data.is_empty() {
        return;
    }

    let msg_type = data[0];
    match msg_type {
        // 0x01: FileChunkSend -- minimum 37 bytes (1 + 16 + 16 + 4), payload follows
        0x01 => {
            if data.len() < 37 {
                warn!(
                    "Binary FileChunkSend too short: {} bytes (need >= 37)",
                    data.len()
                );
                return;
            }
            let target_user_id =
                Uuid::from_bytes(data[1..17].try_into().unwrap());
            let chunk_index = u32::from_be_bytes(data[33..37].try_into().unwrap());
            let payload_len = data.len() - 37;

            if chunk_index % 100 == 0 {
                info!(
                    "Binary relay chunk #{} ({} bytes payload) from {} -> {}",
                    chunk_index, payload_len, sender_user_id, target_user_id
                );
            }

            let mut outgoing = Vec::with_capacity(data.len());
            outgoing.push(0x01);
            outgoing.extend_from_slice(sender_user_id.as_bytes());
            outgoing.extend_from_slice(&data[17..]);

            dispatcher
                .send_binary_to_user(target_user_id, Bytes::from(outgoing))
                .await;
        }

        // 0x02: FileAckSend -- exactly 37 bytes (1 + 16 + 16 + 4)
        0x02 => {
            if data.len() < 37 {
                warn!(
                    "Binary FileAckSend too short: {} bytes (need >= 37)",
                    data.len()
                );
                return;
            }
            let target_user_id =
                Uuid::from_bytes(data[1..17].try_into().unwrap());

            let mut outgoing = Vec::with_capacity(37);
            outgoing.push(0x02);
            outgoing.extend_from_slice(sender_user_id.as_bytes());
            outgoing.extend_from_slice(&data[17..]);

            dispatcher
                .send_binary_to_user(target_user_id, Bytes::from(outgoing))
                .await;
        }

        // 0x03: FileDoneSend -- exactly 33 bytes (1 + 16 + 16)
        0x03 => {
            if data.len() < 33 {
                warn!(
                    "Binary FileDoneSend too short: {} bytes (need >= 33)",
                    data.len()
                );
                return;
            }
            let target_user_id =
                Uuid::from_bytes(data[1..17].try_into().unwrap());

            info!(
                "Binary relay DONE from {} -> {}",
                sender_user_id, target_user_id
            );

            let mut outgoing = Vec::with_capacity(33);
            outgoing.push(0x03);
            outgoing.extend_from_slice(sender_user_id.as_bytes());
            outgoing.extend_from_slice(&data[17..]);

            dispatcher
                .send_binary_to_user(target_user_id, Bytes::from(outgoing))
                .await;
        }

        // #10: 0x04: VoiceAudio binary -- [type(1)] [encrypted_payload...]
        // Server builds [0x04][sender_uid(16)][payload] and relays to all other
        // voice participants as binary frames (no base64 encoding, no JSON).
        0x04 => {
            if data.len() < 2 {
                return; // Need at least type byte + some payload
            }
            let payload = &data[1..];

            // Build outgoing frame: [0x04][sender_uid(16)][payload]
            let mut outgoing = Vec::with_capacity(1 + 16 + payload.len());
            outgoing.push(0x04);
            outgoing.extend_from_slice(sender_user_id.as_bytes());
            outgoing.extend_from_slice(payload);

            dispatcher
                .relay_voice_data_binary(sender_user_id, Bytes::from(outgoing))
                .await;
        }

        _ => {
            warn!("Unknown binary message type: 0x{:02x}", msg_type);
        }
    }
}
