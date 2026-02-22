use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use jsonwebtoken::{DecodingKey, Validation, decode};
use tracing::{info, trace, warn};
use uuid::Uuid;

use haven_types::api::Claims;
use haven_types::events::{GatewayCommand, GatewayEvent};

use crate::dispatcher::{Dispatcher, UserMessage};

/// Heartbeat interval: server sends a Ping every 15 seconds.
/// If 2 consecutive Pongs are missed (~30s), the connection is dropped.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);

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

    // Step 3: Register per-user channel and send existing online users, then go online
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

    // Step 4: Subscribe to broadcasts and relay to this client
    let mut broadcast_rx = dispatcher.subscribe();
    let dispatcher_clone = dispatcher.clone();

    // Per-connection channel subscriptions (shared between send and recv tasks).
    // Events scoped to a channel_id are only forwarded if the user has subscribed.
    // Uses std::sync::RwLock because the critical section is trivial (HashSet lookup)
    // and never held across await points.
    let subscribed_channels: Arc<std::sync::RwLock<HashSet<Uuid>>> =
        Arc::new(std::sync::RwLock::new(HashSet::new()));
    let send_subscriptions = subscribed_channels.clone();

    // H6: Shared flag for heartbeat -- recv_task sets it on Pong, send_task checks it.
    let pong_received = Arc::new(AtomicBool::new(true)); // start as true (just connected)
    let pong_flag_send = pong_received.clone();
    let pong_flag_recv = pong_received.clone();

    // Spawn task to forward broadcasts + targeted messages -> client, with heartbeat (H6)
    let mut send_task = tokio::spawn(async move {
        let mut heartbeat = tokio::time::interval(HEARTBEAT_INTERVAL);
        // Skip the immediate first tick
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

                    // H5: Filter channel-scoped events by subscription.
                    // std::sync::RwLock -- trivial critical section, no await inside.
                    if let Some(channel_id) = msg.channel_id {
                        let subs = send_subscriptions.read()
                            .expect("subscription lock poisoned");
                        if !subs.contains(&channel_id) {
                            continue;
                        }
                    }

                    // Broadcast messages are pre-serialized -- send the JSON directly.
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
                            // Targeted user events are GatewayEvent -- serialize here.
                            let text = serde_json::to_string(&event).unwrap();
                            if sender.send(Message::Text(text.into())).await.is_err() {
                                break;
                            }
                        }
                        UserMessage::Binary(data) => {
                            // Binary relay -- send Bytes directly as WebSocket binary frame.
                            if sender.send(Message::Binary(data)).await.is_err() {
                                break;
                            }
                        }
                    }
                }
                _ = heartbeat.tick() => {
                    // H6: Check if we received a Pong since the last Ping.
                    // Acquire ordering pairs with the Release store in recv_task.
                    if pong_flag_send.swap(false, Ordering::Acquire) {
                        // Pong was received -- reset miss counter
                        missed_heartbeats = 0;
                    } else {
                        missed_heartbeats += 1;
                        if missed_heartbeats >= 2 {
                            warn!("Heartbeat timeout (missed {} pongs), dropping connection", missed_heartbeats);
                            break;
                        }
                    }
                    // Send WebSocket-level ping
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
                // Binary frames: file chunk relay via zero-copy binary protocol.
                Message::Binary(data) => {
                    handle_binary_message(
                        &dispatcher_clone,
                        user_id,
                        &data,
                    ).await;
                }
                // H6: Pong received -- signal the send_task to reset heartbeat counter.
                // Release ordering pairs with the Acquire swap in send_task.
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

async fn handle_command(
    dispatcher: &Dispatcher,
    user_id: Uuid,
    username: &str,
    cmd: GatewayCommand,
    subscriptions: &Arc<std::sync::RwLock<HashSet<Uuid>>>,
) {
    match cmd {
        GatewayCommand::Identify { .. } => {} // Already handled

        // H5: Client subscribes to specific channels
        GatewayCommand::Subscribe { channel_ids } => {
            info!(
                "{} ({}) subscribing to {} channels",
                username,
                user_id,
                channel_ids.len()
            );
            // Update both local connection state and dispatcher state.
            // std::sync::RwLock -- trivial critical section, no await inside.
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

            // Send existing participants to the new joiner so they see who's already here
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
    }
}

/// Handle incoming binary WebSocket frames for the file transfer fast path.
///
/// Binary protocol (all multi-byte integers are big-endian):
///
///   0x01 FileChunkSend:  [type(1)] [target_uid(16)] [transfer_id(16)] [chunk_idx(4)] [payload...]
///   0x02 FileAckSend:    [type(1)] [target_uid(16)] [transfer_id(16)] [ack_chunk_idx(4)]
///   0x03 FileDoneSend:   [type(1)] [target_uid(16)] [transfer_id(16)]
///
/// The server swaps `target_user_id` for `sender_user_id` (from_user_id) and
/// forwards the frame to the target -- zero-copy relay for the encrypted payload.
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

            // Build outgoing frame: replace target_user_id with sender_user_id.
            // Converted to Bytes for O(1) cloning across multiple user connections.
            let mut outgoing = Vec::with_capacity(data.len());
            outgoing.push(0x01);
            outgoing.extend_from_slice(sender_user_id.as_bytes());
            outgoing.extend_from_slice(&data[17..]); // transfer_id + chunk_index + payload

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

        _ => {
            warn!("Unknown binary message type: 0x{:02x}", msg_type);
        }
    }
}
