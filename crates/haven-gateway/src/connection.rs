use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use tracing::{info, trace, warn};
use uuid::Uuid;

use haven_types::api::OfferStatus;
use haven_types::events::{FolderFileEntry, GatewayCommand, GatewayEvent, TurnServer};

use crate::dispatcher::{Dispatcher, UserMessage};

/// Optional database handle for persisting/replaying pending offers.
/// When Some, file/folder offers are stored and replayed on reconnect.
pub type DbHandle = Option<Arc<haven_db::Database>>;

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
    turn_servers: Option<Vec<TurnServer>>,
    db: DbHandle,
) {
    let (mut sender, receiver) = socket.split();

    info!("{} ({}) connected to gateway (pre-authenticated)", username, user_id);

    // Send Ready event
    let ready = GatewayEvent::Ready {
        user_id,
        username: username.clone(),
        turn_servers,
    };
    if sender
        .send(Message::Text(serde_json::to_string(&ready).expect("GatewayEvent serialization").into()))
        .await
        .is_err()
    {
        return;
    }

    // Replay pending offers on reconnect
    if let Some(db) = &db {
        replay_pending_offers(&mut sender, db, user_id, file_server_url.as_deref()).await;
    }

    // Shared connection loop
    run_connection_loop(sender, receiver, dispatcher, user_id, username, file_server_url, db).await;
}

/// Connection event loop — handles broadcasts, targeted messages, and heartbeats.
async fn run_connection_loop(
    mut sender: futures_util::stream::SplitSink<WebSocket, Message>,
    mut receiver: futures_util::stream::SplitStream<WebSocket>,
    dispatcher: Dispatcher,
    user_id: Uuid,
    username: String,
    file_server_url: Option<String>,
    db: DbHandle,
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
            .send(Message::Text(serde_json::to_string(&event).expect("GatewayEvent serialization").into()))
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
    let subscribed_channels: Arc<tokio::sync::RwLock<HashSet<Uuid>>> =
        Arc::new(tokio::sync::RwLock::new(HashSet::new()));
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
                        let subs = send_subscriptions.read().await;
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
                            let text = serde_json::to_string(&event).expect("GatewayEvent serialization");
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
    let db_recv = db;
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
                                &db_recv,
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

async fn handle_command(
    dispatcher: &Dispatcher,
    user_id: Uuid,
    username: &str,
    cmd: GatewayCommand,
    subscriptions: &Arc<tokio::sync::RwLock<HashSet<Uuid>>>,
    file_server_url: Option<&str>,
    db: &DbHandle,
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
                let mut subs = subscriptions.write().await;
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
            folder_id,
        } => {
            info!(
                "{} ({}) -> file offer to {} ({}){}",
                username, user_id, target_user_id, filename,
                if folder_id.is_some() { " [folder]" } else { "" }
            );
            // Persist offer for replay on reconnect
            if let Some(db) = &db {
                let ch_json = chunk_hashes.as_ref().map(|h| serde_json::to_string(h).unwrap_or_default());
                if let Err(e) = db.insert_pending_offer(
                    &transfer_id,
                    &user_id.to_string(),
                    &target_user_id.to_string(),
                    &filename,
                    size as i64,
                    file_sha256.as_deref(),
                    ch_json.as_deref(),
                    file_server_url,
                    folder_id.as_deref(),
                ) {
                    warn!("Failed to persist pending offer: {}", e);
                }
            }
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
                        folder_id,
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
            if let Some(db) = &db {
                let _ = db.update_pending_offer_status(&transfer_id, &OfferStatus::Accepted.to_string());
            }
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
            if let Some(db) = &db {
                let _ = db.update_pending_offer_status(&transfer_id, &OfferStatus::Rejected.to_string());
            }
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
            // Update pending offer with hashes so replayed offers include them
            if let Some(db) = &db {
                if let (Some(sha), Some(hashes)) = (&file_sha256, &chunk_hashes) {
                    let ch_json = serde_json::to_string(hashes).unwrap_or_default();
                    let _ = db.update_pending_offer_hashes(&transfer_id, sha, &ch_json);
                }
                let _ = db.update_pending_offer_status(&transfer_id, &OfferStatus::Uploaded.to_string());
            }
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

        GatewayCommand::FastUploadStart { .. }
        | GatewayCommand::FastNackSend { .. }
        | GatewayCommand::FastDownloadStart { .. } => {
            // These are handled by the file server's WebSocket, not the gateway.
            // If they arrive here, it means the client sent them to the wrong endpoint.
            warn!(
                "{} ({}) received Fast* command on gateway WS — should go to file server",
                username, user_id
            );
        }

        GatewayCommand::FolderOfferSend {
            target_user_id,
            folder_id,
            folder_name,
            total_size,
            file_count,
            manifest,
        } => {
            info!(
                "{} ({}) -> folder offer to {} ({}, {} files, {} bytes)",
                username, user_id, target_user_id, folder_name, file_count, total_size
            );
            if let Some(db) = &db {
                let manifest_json = serde_json::to_string(&manifest).unwrap_or_default();
                if let Err(e) = db.insert_pending_folder_offer(
                    &folder_id,
                    &user_id.to_string(),
                    &target_user_id.to_string(),
                    &folder_name,
                    total_size as i64,
                    file_count as i64,
                    &manifest_json,
                    file_server_url,
                ) {
                    warn!("Failed to persist pending folder offer: {}", e);
                }
            }
            dispatcher
                .send_to_user(
                    target_user_id,
                    GatewayEvent::FolderOffer {
                        from_user_id: user_id,
                        folder_id,
                        folder_name,
                        total_size,
                        file_count,
                        manifest,
                        file_server_url: file_server_url.map(|s| s.to_string()),
                    },
                )
                .await;
        }

        GatewayCommand::FolderAcceptSend {
            target_user_id,
            folder_id,
        } => {
            info!(
                "{} ({}) -> folder accept to {} [folder={}]",
                username, user_id, target_user_id, folder_id
            );
            if let Some(db) = &db {
                let _ = db.update_pending_folder_offer_status(&folder_id, &OfferStatus::Accepted.to_string());
            }
            dispatcher
                .send_to_user(
                    target_user_id,
                    GatewayEvent::FolderAccept {
                        from_user_id: user_id,
                        folder_id,
                    },
                )
                .await;
        }

        GatewayCommand::FolderRejectSend {
            target_user_id,
            folder_id,
        } => {
            info!(
                "{} ({}) -> folder reject to {} [folder={}]",
                username, user_id, target_user_id, folder_id
            );
            if let Some(db) = &db {
                let _ = db.update_pending_folder_offer_status(&folder_id, &OfferStatus::Rejected.to_string());
            }
            dispatcher
                .send_to_user(
                    target_user_id,
                    GatewayEvent::FolderReject {
                        from_user_id: user_id,
                        folder_id,
                    },
                )
                .await;
        }

        GatewayCommand::FastProgressSend {
            target_user_id,
            transfer_id,
            bytes_done,
            bytes_total,
        } => {
            // Relay upload progress from sender to receiver
            dispatcher
                .send_to_user(
                    target_user_id,
                    GatewayEvent::FastProgress {
                        from_user_id: user_id,
                        transfer_id,
                        bytes_done,
                        bytes_total,
                    },
                )
                .await;
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
///   0x05 ScreenAudio:    [type(1)] [encrypted_payload...]
///
/// For 0x01-0x03: The server swaps `target_user_id` for `sender_user_id`
/// and forwards the frame to the target — zero-copy relay for the encrypted payload.
///
/// For 0x04-0x05: The server prepends the sender's UUID and relays to all other
/// voice channel participants as binary frames. 0x05 is screen share system audio
/// (48kHz stereo) routed to a separate playback pipeline on receivers.
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
        // 0x01-0x03: File transfer relay -- swap target_uid for sender_uid, forward.
        0x01 | 0x02 | 0x03 => {
            let (label, min_len) = match msg_type {
                0x01 => ("FileChunkSend", 37),
                0x02 => ("FileAckSend", 37),
                0x03 => ("FileDoneSend", 33),
                _ => unreachable!(),
            };
            if data.len() < min_len {
                warn!("Binary {} too short: {} bytes (need >= {})", label, data.len(), min_len);
                return;
            }
            let target_user_id = Uuid::from_bytes(data[1..17].try_into().unwrap());

            if msg_type == 0x01 {
                let chunk_index = u32::from_be_bytes(data[33..37].try_into().unwrap());
                if chunk_index % 100 == 0 {
                    info!("Binary relay chunk #{} ({} bytes payload) from {} -> {}",
                        chunk_index, data.len() - 37, sender_user_id, target_user_id);
                }
            } else if msg_type == 0x03 {
                info!("Binary relay DONE from {} -> {}", sender_user_id, target_user_id);
            }

            let outgoing = relay_binary_frame(msg_type, sender_user_id, &data[17..]);
            dispatcher.send_binary_to_user(target_user_id, outgoing).await;
        }

        // 0x04/0x05: Voice/ScreenAudio binary relay to all voice participants.
        0x04 | 0x05 => {
            if data.len() < 2 {
                return;
            }
            let outgoing = relay_binary_frame(msg_type, sender_user_id, &data[1..]);
            dispatcher.relay_voice_data_binary(sender_user_id, outgoing).await;
        }

        _ => {
            warn!("Unknown binary message type: 0x{:02x}", msg_type);
        }
    }
}

/// Build an outgoing binary frame: [msg_type][sender_uid(16)][tail_data].
/// Used by all binary relay handlers to swap target_uid for sender_uid.
fn relay_binary_frame(msg_type: u8, sender_user_id: Uuid, tail_data: &[u8]) -> Bytes {
    let mut outgoing = Vec::with_capacity(1 + 16 + tail_data.len());
    outgoing.push(msg_type);
    outgoing.extend_from_slice(sender_user_id.as_bytes());
    outgoing.extend_from_slice(tail_data);
    Bytes::from(outgoing)
}

/// Replay pending file/folder offers to a reconnecting client.
async fn replay_pending_offers(
    sender: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    db: &haven_db::Database,
    user_id: Uuid,
    file_server_url: Option<&str>,
) {
    let uid = user_id.to_string();

    // Replay folder offers
    if let Ok(folders) = db.get_pending_folder_offers_for_user(&uid) {
        for f in folders {
            let manifest: Vec<FolderFileEntry> = serde_json::from_str(&f.manifest).unwrap_or_default();
            let folder_id = f.folder_id;
            let event = GatewayEvent::FolderOffer {
                from_user_id: f.from_user_id.parse().unwrap_or(Uuid::nil()),
                folder_id: folder_id.clone(),
                folder_name: f.folder_name,
                total_size: f.total_size as u64,
                file_count: f.file_count as u32,
                manifest,
                file_server_url: f.file_server_url.or_else(|| file_server_url.map(|s| s.to_string())),
            };
            let text = serde_json::to_string(&event).expect("GatewayEvent serialization");
            if sender.send(Message::Text(text.into())).await.is_err() {
                return;
            }
            info!("Replayed pending folder offer {} to {}", folder_id, uid);
        }
    }

    // Replay file offers
    if let Ok(offers) = db.get_pending_offers_for_user(&uid) {
        for o in offers {
            let chunk_hashes: Option<Vec<String>> = o.chunk_hashes.as_ref()
                .and_then(|j| serde_json::from_str(j).ok());
            let event = GatewayEvent::FileOffer {
                from_user_id: o.from_user_id.parse().unwrap_or(Uuid::nil()),
                transfer_id: o.transfer_id.clone(),
                filename: o.filename,
                size: o.file_size as u64,
                file_sha256: o.file_sha256.clone(),
                chunk_hashes: chunk_hashes.clone(),
                file_server_url: o.file_server_url.or_else(|| file_server_url.map(|s| s.to_string())),
                folder_id: o.folder_id,
            };
            let text = serde_json::to_string(&event).expect("GatewayEvent serialization");
            if sender.send(Message::Text(text.into())).await.is_err() {
                return;
            }

            // If upload is complete, also replay FileReady
            if o.status == OfferStatus::Uploaded.to_string() && o.file_sha256.is_some() && chunk_hashes.is_some() {
                let ready = GatewayEvent::FileReady {
                    from_user_id: o.from_user_id.parse().unwrap_or(Uuid::nil()),
                    transfer_id: o.transfer_id.clone(),
                    file_server_url: file_server_url.map(|s| s.to_string()),
                    file_sha256: o.file_sha256,
                    chunk_hashes,
                };
                let text = serde_json::to_string(&ready).expect("GatewayEvent serialization");
                if sender.send(Message::Text(text.into())).await.is_err() {
                    return;
                }
            }
            info!("Replayed pending offer {} to {}", o.transfer_id, uid);
        }
    }
}
