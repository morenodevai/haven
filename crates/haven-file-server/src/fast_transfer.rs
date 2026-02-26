/// Fast transfer WebSocket handler for the file server.
///
/// Handles FastUploadStart / FastDownloadStart commands from clients,
/// manages UDP receiver/sender pipelines, and sends control messages
/// (FastNack, FastChunkAck, FastUploadDone, FastDownloadDone) back.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use crossbeam_channel::bounded;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use haven_fast_transfer::{
    NackMessage, ChunkAckMessage, RawSenderConfig, ReceiverConfig, ReceiverProgress,
    SenderProgress, TracingLogger, run_raw_sender, run_receiver,
};

use crate::routes::{AppState, Claims};

/// WebSocket control messages for fast transfer (JSON, tagged union).
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum FastControlMessage {
    // Client → Server
    FastUploadStart {
        transfer_id: String,
        file_size: u64,
        chunk_count: u32,
        chunk_size: u64,
        chunk_hashes: Vec<String>,
        file_sha256: String,
    },
    FastDownloadStart {
        transfer_id: String,
        udp_port: u16,
    },

    // Server → Client
    FastUploadReady {
        transfer_id: String,
        udp_port: u16,
    },
    FastNack {
        transfer_id: String,
        chunk_idx: u32,
        missing_frames: Vec<u16>,
    },
    FastChunkAck {
        transfer_id: String,
        chunk_idx: u32,
    },
    FastUploadDone {
        transfer_id: String,
    },
    FastDownloadReady {
        transfer_id: String,
    },
    FastDownloadDone {
        transfer_id: String,
    },
}

/// Handle a fast transfer WebSocket connection.
///
/// This endpoint handles both upload and download control signaling.
/// The JWT is passed as a query parameter: `/fast-transfer?token=...`
pub async fn handle_fast_transfer_ws(
    socket: WebSocket,
    state: AppState,
    claims: Claims,
    peer_addr: SocketAddr,
) {
    let (mut ws_tx, mut ws_rx) = socket.split();

    info!("Fast transfer WS connected: user={} peer={}", claims.username, peer_addr);

    while let Some(Ok(msg)) = ws_rx.next().await {
        let text = match msg {
            Message::Text(t) => t,
            Message::Close(_) => break,
            _ => continue,
        };

        let ctrl: FastControlMessage = match serde_json::from_str(&text) {
            Ok(c) => c,
            Err(e) => {
                warn!("Bad fast transfer message: {}", e);
                continue;
            }
        };

        match ctrl {
            FastControlMessage::FastUploadStart {
                transfer_id,
                file_size,
                chunk_count,
                chunk_size,
                chunk_hashes,
                file_sha256,
            } => {
                info!(
                    "FastUploadStart: transfer={} size={} chunks={}",
                    transfer_id, file_size, chunk_count
                );

                // Create transfer record in DB
                let retention_hours = state.retention_hours;
                let tid = transfer_id.clone();
                let uploader_id = claims.sub.clone();
                let hashes = chunk_hashes.clone();
                let cs = chunk_size;
                let fs = file_size;
                let fsha = file_sha256.clone();

                let db_result = state.db.with_conn_mut(move |conn| {
                    conn.execute(
                        "INSERT INTO transfers (id, uploader_id, file_size, chunk_size, chunk_count, file_sha256, expires_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now', '+' || ?7 || ' hours'))",
                        rusqlite::params![
                            &tid, &uploader_id, fs as i64, cs as i64,
                            chunk_count as i64, &fsha, retention_hours as i64,
                        ],
                    )?;

                    let mut offset: u64 = 0;
                    for (i, hash) in hashes.iter().enumerate() {
                        let length = if i == chunk_count as usize - 1 {
                            fs - offset
                        } else {
                            cs
                        };
                        conn.execute(
                            "INSERT INTO chunks (transfer_id, chunk_index, sha256, byte_offset, byte_length)
                             VALUES (?1, ?2, ?3, ?4, ?5)",
                            rusqlite::params![&tid, i as i64, hash, offset as i64, length as i64],
                        )?;
                        offset += length;
                    }
                    Ok(())
                });

                if let Err(e) = db_result {
                    warn!("FastUploadStart DB error: {}", e);
                    continue;
                }

                // Pre-allocate file
                if let Err(e) = state.storage.create_file(&transfer_id, file_size).await {
                    warn!("FastUploadStart storage error: {}", e);
                    continue;
                }

                // Use the shared UDP socket (fixed port, bound at startup)
                let udp_socket = match state.udp_socket.try_clone() {
                    Ok(s) => s,
                    Err(e) => {
                        warn!("FastUploadStart socket clone error: {}", e);
                        continue;
                    }
                };
                let udp_port = state.udp_port;

                info!("Fast upload using fixed UDP port {}", udp_port);

                // Send FastUploadReady BEFORE starting receiver
                let ready = FastControlMessage::FastUploadReady {
                    transfer_id: transfer_id.clone(),
                    udp_port,
                };
                let _ = ws_tx
                    .send(Message::Text(serde_json::to_string(&ready).unwrap().into()))
                    .await;

                // Now start receiver pipeline with the pre-bound socket
                let output_path = state.storage.file_path(&transfer_id);
                let transfer_id_bytes = parse_transfer_id_bytes(&transfer_id);
                let logger = Arc::new(TracingLogger);

                let receiver_config = ReceiverConfig {
                    output_path: output_path.to_string_lossy().into_owned(),
                    transfer_id: transfer_id_bytes,
                    file_size,
                    chunk_count,
                    chunk_size,
                    chunk_hashes: chunk_hashes.clone(),
                    file_sha256: file_sha256.clone(),
                    bind_addr: format!("0.0.0.0:{}", udp_port).parse().unwrap(),
                    logger: Some(logger),
                    pre_bound_socket: Some(udp_socket),
                };

                let progress = Arc::new(ReceiverProgress::new());
                let progress_clone = progress.clone();

                // Channel to collect NACKs from receiver → WS sender
                let (nack_tx, nack_rx) = bounded::<(u32, Vec<u16>)>(256);

                let nack_callback: haven_fast_transfer::NackCallback =
                    Box::new(move |chunk_idx, missing| {
                        let _ = nack_tx.try_send((chunk_idx, missing));
                    });

                let tid_complete = transfer_id.clone();
                let db_complete = state.db.clone();

                // Start receiver in a blocking thread
                let receiver_handle = std::thread::spawn(move || {
                    run_receiver(receiver_config, progress_clone, nack_callback)
                });

                // Event loop: forward NACKs to client, read WS messages, detect completion
                let tid_ws = transfer_id.clone();
                let progress_poll = progress.clone();
                loop {
                    // Check for NACKs from receiver (non-blocking batch drain)
                    let mut nacks_sent = 0;
                    while let Ok((chunk_idx, missing)) = nack_rx.try_recv() {
                        let nack_msg = FastControlMessage::FastNack {
                            transfer_id: tid_ws.clone(),
                            chunk_idx,
                            missing_frames: missing,
                        };
                        if ws_tx.send(Message::Text(serde_json::to_string(&nack_msg).unwrap().into())).await.is_err() {
                            warn!("WS send failed for NACK chunk {}", chunk_idx);
                            break;
                        }
                        nacks_sent += 1;
                        if nacks_sent >= 50 { break; } // don't starve the loop
                    }

                    // Check receiver state
                    let recv_state = progress_poll.state.load(std::sync::atomic::Ordering::Relaxed);
                    if recv_state == haven_fast_transfer::receiver::STATE_COMPLETE {
                        // Send FastUploadDone
                        let done = FastControlMessage::FastUploadDone {
                            transfer_id: tid_ws.clone(),
                        };
                        let _ = ws_tx.send(Message::Text(serde_json::to_string(&done).unwrap().into())).await;
                        info!("Fast upload receiver complete, sent FastUploadDone");
                        break;
                    }
                    if recv_state == haven_fast_transfer::receiver::STATE_ERROR {
                        warn!("Fast upload receiver error state");
                        break;
                    }

                    // Brief yield to not busy-spin
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }

                // Wait for receiver thread to finish and update DB
                tokio::task::spawn_blocking(move || {
                    match receiver_handle.join() {
                        Ok(Ok(_)) => {
                            let _ = db_complete.with_conn_mut(|conn| {
                                conn.execute(
                                    "UPDATE transfers SET status = 'complete', bytes_received = file_size WHERE id = ?1",
                                    [&tid_complete],
                                )?;
                                Ok(())
                            });
                            info!("Fast upload complete: {}", tid_complete);
                        }
                        Ok(Err(e)) => {
                            warn!("Fast upload failed: {}: {}", tid_complete, e);
                        }
                        Err(_) => {
                            warn!("Fast upload receiver panicked: {}", tid_complete);
                        }
                    }
                });

                // Upload session done, break out of main WS loop
                break;
            }

            FastControlMessage::FastDownloadStart {
                transfer_id,
                udp_port,
            } => {
                info!(
                    "FastDownloadStart: transfer={} receiver_port={}",
                    transfer_id, udp_port
                );

                // Look up transfer metadata
                let transfer_info: Option<(u64, u64, String, String)> = state
                    .db
                    .with_conn(|conn| {
                        conn.query_row(
                            "SELECT file_size, chunk_size, file_sha256, status FROM transfers WHERE id = ?1",
                            [&transfer_id],
                            |row| {
                                Ok((
                                    row.get::<_, i64>(0)? as u64,
                                    row.get::<_, i64>(1)? as u64,
                                    row.get::<_, String>(2)?,
                                    row.get::<_, String>(3)?,
                                ))
                            },
                        )
                        .map_err(|_| anyhow::anyhow!("Transfer not found"))
                    })
                    .ok();

                let (file_size, chunk_size, _file_sha256, status) = match transfer_info {
                    Some(info) => info,
                    None => {
                        warn!("FastDownloadStart: transfer {} not found", transfer_id);
                        continue;
                    }
                };

                if status != "complete" {
                    warn!(
                        "FastDownloadStart: transfer {} not complete (status={})",
                        transfer_id, status
                    );
                    continue;
                }

                // Get chunk count
                let chunk_count = if file_size == 0 {
                    1u32
                } else {
                    ((file_size as f64 / chunk_size as f64).ceil()) as u32
                };

                // Target address: peer's IP + their specified UDP port
                let target_addr = SocketAddr::new(peer_addr.ip(), udp_port);

                let file_path = state.storage.file_path(&transfer_id);
                let transfer_id_bytes = parse_transfer_id_bytes(&transfer_id);

                info!(
                    "Starting download blast: {} ({} bytes, {} chunks) to {}",
                    transfer_id, file_size, chunk_count, target_addr
                );

                // Send FastDownloadReady
                let ready = FastControlMessage::FastDownloadReady {
                    transfer_id: transfer_id.clone(),
                };
                let _ = ws_tx
                    .send(Message::Text(serde_json::to_string(&ready).unwrap().into()))
                    .await;

                // Set up NACK/ACK channels for the sender
                let (nack_tx, nack_rx) = bounded::<NackMessage>(256);
                let (ack_tx, ack_rx) = bounded::<ChunkAckMessage>(256);

                let logger = Arc::new(TracingLogger);
                let sender_config = RawSenderConfig {
                    file_path: file_path.to_string_lossy().into_owned(),
                    target_addr,
                    transfer_id: transfer_id_bytes,
                    file_size,
                    chunk_size,
                    chunk_count,
                    logger: Some(logger),
                };

                let sender_progress = Arc::new(SenderProgress::new());
                let tid_done = transfer_id.clone();

                // Start sender in blocking thread
                let sender_handle = std::thread::spawn(move || {
                    run_raw_sender(sender_config, sender_progress, nack_rx, ack_rx)
                });

                // Process incoming WS messages (NACKs from client) while sender runs
                let nack_tx_clone = nack_tx;
                let ack_tx_clone = ack_tx;

                // Read NACKs/ACKs from WS and feed to sender
                while let Some(Ok(msg)) = ws_rx.next().await {
                    let text = match msg {
                        Message::Text(t) => t,
                        Message::Close(_) => break,
                        _ => continue,
                    };

                    if let Ok(ctrl) = serde_json::from_str::<FastControlMessage>(&text) {
                        match ctrl {
                            FastControlMessage::FastNack {
                                chunk_idx,
                                missing_frames,
                                ..
                            } => {
                                let _ = nack_tx_clone.try_send(NackMessage {
                                    chunk_index: chunk_idx,
                                    missing_frames,
                                });
                            }
                            FastControlMessage::FastChunkAck { chunk_idx, .. } => {
                                let _ = ack_tx_clone.try_send(ChunkAckMessage {
                                    chunk_index: chunk_idx,
                                });
                            }
                            _ => {}
                        }
                    }
                }

                // WS closed or sender finished — wait for sender
                tokio::task::spawn_blocking(move || {
                    match sender_handle.join() {
                        Ok(Ok(_)) => {
                            info!("Fast download blast complete: {}", tid_done);
                        }
                        Ok(Err(e)) => {
                            warn!("Fast download blast failed: {}: {}", tid_done, e);
                        }
                        Err(_) => {
                            warn!("Fast download sender panicked: {}", tid_done);
                        }
                    }
                });

                // Send FastDownloadDone (best effort — WS may already be closed)
                let done = FastControlMessage::FastDownloadDone {
                    transfer_id: transfer_id.clone(),
                };
                let _ = ws_tx
                    .send(Message::Text(serde_json::to_string(&done).unwrap().into()))
                    .await;

                // Break out of the main loop — download session is done
                break;
            }

            _ => {
                warn!("Unexpected fast transfer message from client");
            }
        }
    }

    info!("Fast transfer WS disconnected: user={}", claims.username);
}

/// Parse a transfer ID string into 16 bytes (UUID without hyphens, or truncated hash).
fn parse_transfer_id_bytes(transfer_id: &str) -> [u8; 16] {
    let stripped = transfer_id.replace('-', "");
    if stripped.len() >= 32 {
        if let Ok(bytes) = hex::decode(&stripped[..32]) {
            let mut arr = [0u8; 16];
            arr.copy_from_slice(&bytes);
            return arr;
        }
    }
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(transfer_id.as_bytes());
    let mut arr = [0u8; 16];
    arr.copy_from_slice(&hash[..16]);
    arr
}
