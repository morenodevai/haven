/// Fast UDP blast upload using haven-fast-transfer.
///
/// Replaces the two-pass HTTP upload with a single-pass UDP blast:
/// 1. Start sender pipeline (reader → encryptor → blaster)
/// 2. Connect to file server's fast-transfer WebSocket
/// 3. Send FastUploadStart with metadata
/// 4. Receive FastUploadReady with UDP port
/// 5. Blast encrypted chunks via UDP
/// 6. Handle NACKs via WebSocket → crossbeam channel → sender retransmit

use std::sync::Arc;
use std::sync::atomic::Ordering;

use crossbeam_channel::bounded;

use haven_fast_transfer::{
    SenderConfig, SenderProgress, run_sender,
    NackMessage, ChunkAckMessage, TracingLogger,
};

use crate::crypto::derive_key;
use crate::upload::{UploadProgress, STATE_HASHING, STATE_UPLOADING, STATE_COMPLETE, STATE_ERROR, STATE_CANCELLED};

/// Run a fast UDP blast upload.
///
/// This function is called from the FFI layer and runs on a Tokio runtime.
pub async fn fast_upload_file(
    file_path: &str,
    file_server_url: &str,
    transfer_id: &str,
    jwt_token: &str,
    master_key: &[u8],
    salt: &[u8],
    progress: Arc<UploadProgress>,
) -> Result<(), String> {
    let key = derive_key(master_key, salt);

    let file_size = tokio::fs::metadata(file_path)
        .await
        .map_err(|e| format!("Cannot read file: {}", e))?
        .len();

    progress.bytes_total.store(file_size, Ordering::Relaxed);
    progress.state.store(STATE_HASHING, Ordering::Relaxed);

    // Parse transfer_id to 16 bytes
    let transfer_id_bytes = parse_transfer_id_bytes(transfer_id);

    // Channels for NACK/ACK communication between WS and sender
    let (nack_tx, nack_rx) = bounded::<NackMessage>(256);
    let (ack_tx, ack_rx) = bounded::<ChunkAckMessage>(256);

    // We need to connect to the file server's fast-transfer WebSocket first
    // to get the UDP port, then start the sender pipeline.

    // Connect to file server WS
    let ws_url = format!(
        "{}/fast-transfer?token={}",
        file_server_url.replace("http://", "ws://").replace("https://", "wss://"),
        jwt_token
    );

    let (ws_stream, _) = tokio_tungstenite::connect_async(&ws_url)
        .await
        .map_err(|e| format!("WS connect failed: {}", e))?;

    let (mut ws_tx, mut ws_rx) = futures_util::StreamExt::split(ws_stream);

    // Send FastUploadStart
    let _chunk_size = haven_fast_transfer::CHUNK_SIZE as u64;
    let encrypted_chunk_size = haven_fast_transfer::ENCRYPTED_CHUNK_SIZE as u64;
    let chunk_count = if file_size == 0 {
        1u32
    } else {
        ((file_size as usize + haven_fast_transfer::CHUNK_SIZE - 1) / haven_fast_transfer::CHUNK_SIZE) as u32
    };

    // We need to compute hashes first (pass 1) before we can send FastUploadStart.
    // The sender pipeline does this, but we need hashes before blasting.
    // So we run the sender and it will produce hashes in progress.hashes_json.

    let file_path_owned = file_path.to_string();
    let transfer_id_owned = transfer_id.to_string();
    let _file_server_url_owned = file_server_url.to_string();
    let _jwt_token_owned = jwt_token.to_string();

    // We need to know the target UDP address before starting the sender.
    // The flow is:
    // 1. Compute hashes (sender pipeline does this)
    // 2. Send FastUploadStart with hashes
    // 3. Receive FastUploadReady with UDP port
    // 4. Start blasting

    // For now, use a phased approach:
    // Phase A: Run the sender pipeline which encrypts and hashes
    // Phase B: Read hashes, send FastUploadStart, get port
    // Phase C: Sender blasts to that port

    // Actually, the sender pipeline does read→encrypt→blast as one pipeline.
    // We need to split it differently or use a two-step approach.
    // Let's create a modified flow:

    // Step 1: Pre-compute hashes using the sender's internal logic
    // Step 2: Send FastUploadStart, get UDP port
    // Step 3: Run sender pipeline targeting that port

    // Pre-compute hashes (single pass, same as current upload.rs pass 1)
    let (chunk_hashes, file_sha256, encrypted_size) = {
        let file_path_hash = file_path_owned.clone();
        let progress_hash = progress.clone();

        tokio::task::block_in_place(|| -> Result<(Vec<String>, String, u64), String> {
            use std::io::Read;
            use sha2::{Sha256, Digest};
            use aes_gcm::{Aes256Gcm, KeyInit, Nonce, aead::Aead};

            let mut file = std::fs::File::open(&file_path_hash)
                .map_err(|e| format!("Cannot open file: {}", e))?;

            let cipher = Aes256Gcm::new_from_slice(&key)
                .map_err(|e| format!("Cipher init: {}", e))?;

            let mut full_hasher = Sha256::new();
            let mut chunk_hashes = Vec::with_capacity(chunk_count as usize);
            let mut encrypted_size: u64 = 0;
            let mut buf = vec![0u8; haven_fast_transfer::CHUNK_SIZE];

            for idx in 0..chunk_count {
                if progress_hash.is_cancelled() {
                    return Err("Cancelled".into());
                }

                let remaining = file_size - idx as u64 * haven_fast_transfer::CHUNK_SIZE as u64;
                let to_read = (remaining as usize).min(haven_fast_transfer::CHUNK_SIZE);

                file.read_exact(&mut buf[..to_read])
                    .map_err(|e| format!("Read error chunk {}: {}", idx, e))?;

                let nonce = crate::crypto::derive_chunk_nonce(&key, idx as u64);
                let ciphertext = cipher
                    .encrypt(Nonce::from_slice(&nonce), &buf[..to_read])
                    .map_err(|e| format!("Encrypt chunk {}: {}", idx, e))?;

                let mut encrypted = Vec::with_capacity(12 + ciphertext.len());
                encrypted.extend_from_slice(&nonce);
                encrypted.extend_from_slice(&ciphertext);

                let mut chunk_hasher = Sha256::new();
                chunk_hasher.update(&encrypted);
                chunk_hashes.push(hex::encode(chunk_hasher.finalize()));

                full_hasher.update(&encrypted);
                encrypted_size += encrypted.len() as u64;
            }

            Ok((chunk_hashes, hex::encode(full_hasher.finalize()), encrypted_size))
        })?
    };

    if progress.is_cancelled() {
        progress.state.store(STATE_CANCELLED, Ordering::Relaxed);
        return Err("Cancelled".into());
    }

    // Store hashes for Dart to read
    {
        let json = serde_json::json!({
            "file_sha256": file_sha256,
            "chunk_hashes": chunk_hashes,
        }).to_string();
        *progress.hashes_json.lock().unwrap() = Some(json);
    }

    // Switch to uploading state
    progress.bytes_total.store(encrypted_size, Ordering::Relaxed);
    progress.bytes_done.store(0, Ordering::Relaxed);
    progress.state.store(STATE_UPLOADING, Ordering::Relaxed);

    // Send FastUploadStart
    let start_msg = serde_json::json!({
        "type": "FastUploadStart",
        "data": {
            "transfer_id": transfer_id_owned,
            "file_size": encrypted_size,
            "chunk_count": chunk_count,
            "chunk_size": encrypted_chunk_size,
            "chunk_hashes": chunk_hashes,
            "file_sha256": file_sha256,
        }
    });

    use futures_util::SinkExt;
    ws_tx
        .send(tokio_tungstenite::tungstenite::Message::Text(start_msg.to_string()))
        .await
        .map_err(|e| format!("WS send error: {}", e))?;

    // Wait for FastUploadReady
    let udp_port: u16 = loop {
        use futures_util::StreamExt;
        match ws_rx.next().await {
            Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => {
                if let Ok(msg) = serde_json::from_str::<serde_json::Value>(&text) {
                    if msg["type"] == "FastUploadReady" {
                        let port = msg["data"]["udp_port"].as_u64().unwrap_or(0) as u16;
                        if port > 0 {
                            break port;
                        }
                    }
                }
            }
            Some(Ok(_)) => continue,
            _ => return Err("WS connection lost waiting for FastUploadReady".into()),
        }
    };

    // Parse file server address and replace port with UDP port
    let server_addr: std::net::SocketAddr = {
        let url = url::Url::parse(file_server_url)
            .map_err(|_| format!("Invalid server URL: {}", file_server_url))?;
        let host = url.host_str().unwrap_or("127.0.0.1");
        format!("{}:{}", host, udp_port)
            .parse()
            .map_err(|e| format!("Cannot parse target addr: {}", e))?
    };

    // Start the sender pipeline in a blocking thread
    let sender_config = SenderConfig {
        file_path: file_path_owned.clone(),
        target_addr: server_addr,
        transfer_id: transfer_id_bytes,
        encryption_key: key,
        logger: Some(Arc::new(TracingLogger)),
    };

    let sender_progress = Arc::new(SenderProgress::new());
    let sender_progress_clone = sender_progress.clone();
    let progress_poll = progress.clone();

    // Spawn WS reader to feed NACKs and ACKs to sender
    let nack_tx_clone = nack_tx.clone();
    let ack_tx_clone = ack_tx.clone();
    tokio::spawn(async move {
        use futures_util::StreamExt;
        while let Some(Ok(msg)) = ws_rx.next().await {
            if let tokio_tungstenite::tungstenite::Message::Text(text) = msg {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                    match v["type"].as_str() {
                        Some("FastNack") => {
                            if let (Some(chunk_idx), Some(frames)) = (
                                v["data"]["chunk_idx"].as_u64(),
                                v["data"]["missing_frames"].as_array(),
                            ) {
                                let missing: Vec<u16> = frames
                                    .iter()
                                    .filter_map(|f| f.as_u64().map(|n| n as u16))
                                    .collect();
                                let _ = nack_tx_clone.try_send(NackMessage {
                                    chunk_index: chunk_idx as u32,
                                    missing_frames: missing,
                                });
                            }
                        }
                        Some("FastChunkAck") => {
                            if let Some(chunk_idx) = v["data"]["chunk_idx"].as_u64() {
                                let _ = ack_tx_clone.try_send(ChunkAckMessage {
                                    chunk_index: chunk_idx as u32,
                                });
                            }
                        }
                        Some("FastUploadDone") => {
                            break;
                        }
                        _ => {}
                    }
                }
            }
        }
    });

    // Run sender in blocking thread, poll progress to update UploadProgress
    let sender_handle = tokio::task::spawn_blocking(move || {
        run_sender(sender_config, sender_progress_clone, nack_rx, ack_rx)
    });

    // Poll sender progress and copy to upload progress
    let poll_handle = tokio::spawn(async move {
        loop {
            let state = sender_progress.state.load(Ordering::Relaxed);
            progress_poll.bytes_done.store(
                sender_progress.bytes_done.load(Ordering::Relaxed),
                Ordering::Relaxed,
            );

            if state == haven_fast_transfer::sender::STATE_COMPLETE
                || state == haven_fast_transfer::sender::STATE_ERROR
                || state == haven_fast_transfer::sender::STATE_CANCELLED
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    });

    let result = sender_handle
        .await
        .map_err(|e| format!("Sender task panicked: {}", e))?;

    poll_handle.abort();

    match result {
        Ok(_send_result) => {
            progress.state.store(STATE_COMPLETE, Ordering::Relaxed);
            Ok(())
        }
        Err(e) => {
            progress.state.store(STATE_ERROR, Ordering::Relaxed);
            Err(e)
        }
    }
}

/// Parse transfer ID (UUID string) into 16 bytes.
fn parse_transfer_id_bytes(transfer_id: &str) -> [u8; 16] {
    let stripped = transfer_id.replace('-', "");
    if stripped.len() >= 32 {
        if let Ok(bytes) = hex::decode(&stripped[..32]) {
            let mut arr = [0u8; 16];
            arr.copy_from_slice(&bytes);
            return arr;
        }
    }
    use sha2::{Sha256, Digest};
    let hash = Sha256::digest(transfer_id.as_bytes());
    let mut arr = [0u8; 16];
    arr.copy_from_slice(&hash[..16]);
    arr
}
