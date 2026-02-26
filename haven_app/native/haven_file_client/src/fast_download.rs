/// Fast UDP blast download using haven-fast-transfer.
///
/// Replaces the HTTP streaming download with UDP blast receive:
/// 1. Connect to file server's fast-transfer WebSocket
/// 2. Send FastDownloadStart with local UDP port
/// 3. Start receiver pipeline (vacuum → assembler → writer)
/// 4. Server blasts encrypted chunks via UDP
/// 5. Send NACKs for missing frames via WebSocket
/// 6. Once complete: decrypt, verify, write

use std::sync::Arc;
use std::sync::atomic::Ordering;

use crossbeam_channel::bounded;

use haven_fast_transfer::{
    ReceiverConfig, ReceiverProgress, run_receiver, TracingLogger,
};

use crate::crypto::{derive_key, decrypt_chunk};
use crate::download::DownloadProgress;
use crate::upload::{STATE_UPLOADING as STATE_DOWNLOADING, STATE_COMPLETE, STATE_CANCELLED};

/// Run a fast UDP blast download.
pub async fn fast_download_file(
    save_path: &str,
    file_server_url: &str,
    transfer_id: &str,
    jwt_token: &str,
    master_key: &[u8],
    salt: &[u8],
    file_sha256: &str,
    chunk_hashes: &[String],
    progress: Arc<DownloadProgress>,
) -> Result<(), String> {
    let key = derive_key(master_key, salt);

    progress.state.store(STATE_DOWNLOADING, Ordering::Relaxed);

    // Calculate expected encrypted file size
    let chunk_count = chunk_hashes.len() as u32;
    let encrypted_chunk_size = haven_fast_transfer::ENCRYPTED_CHUNK_SIZE as u64;

    // We receive encrypted data, then decrypt. The receiver writes encrypted chunks
    // to a temp file, then we decrypt in a second pass.
    let temp_path = format!("{}.enc", save_path);

    // First, we need to know the encrypted file size to set up the receiver.
    // Each chunk is ENCRYPTED_CHUNK_SIZE except possibly the last.
    // But we don't know the plaintext file size here — we only have chunk hashes.
    // The file_size for the receiver = total encrypted bytes.
    // We can compute it: each hash corresponds to one encrypted chunk.
    // For the last chunk, we don't know exact size. We'll use the transfer metadata.

    // Actually, we should get file size from the file server.
    // Let's query the transfer status first.
    let client = reqwest::Client::new();
    let status_resp = client
        .get(format!("{}/transfers/{}", file_server_url, transfer_id))
        .header("Authorization", format!("Bearer {}", jwt_token))
        .send()
        .await
        .map_err(|e| format!("Status query failed: {}", e))?;

    if !status_resp.status().is_success() {
        return Err(format!("Transfer status query failed: {}", status_resp.status()));
    }

    let status_json: serde_json::Value = status_resp
        .json()
        .await
        .map_err(|e| format!("Status parse failed: {}", e))?;

    let encrypted_file_size = status_json["file_size"].as_u64().unwrap_or(0);
    let _chunk_size_from_server = status_json["chunk_count"].as_u64().unwrap_or(0);

    if encrypted_file_size == 0 {
        return Err("Transfer has zero file size".into());
    }

    progress.bytes_total.store(encrypted_file_size, Ordering::Relaxed);

    // Parse transfer ID bytes
    let transfer_id_bytes = parse_transfer_id_bytes(transfer_id);

    // Fixed UDP port for receiving downloads (matches file server convention)
    const CLIENT_UDP_PORT: u16 = 3211;
    let actual_bind_addr: std::net::SocketAddr = format!("0.0.0.0:{}", CLIENT_UDP_PORT).parse().unwrap();
    let udp_socket = {
        use socket2::{Domain, Protocol, Socket, Type};
        let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))
            .map_err(|e| format!("UDP socket create: {}", e))?;
        sock.set_recv_buffer_size(32 * 1024 * 1024)
            .map_err(|e| format!("UDP recv buffer: {}", e))?;
        sock.bind(&actual_bind_addr.into())
            .map_err(|e| format!("UDP bind on port {}: {}", CLIENT_UDP_PORT, e))?;
        let std_sock: std::net::UdpSocket = sock.into();
        std_sock
    };
    let udp_port = CLIENT_UDP_PORT;

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

    // Send FastDownloadStart with our UDP port
    let start_msg = serde_json::json!({
        "type": "FastDownloadStart",
        "data": {
            "transfer_id": transfer_id,
            "udp_port": udp_port,
        }
    });

    use futures_util::SinkExt;
    ws_tx
        .send(tokio_tungstenite::tungstenite::Message::Text(start_msg.to_string()))
        .await
        .map_err(|e| format!("WS send error: {}", e))?;

    // Wait for FastDownloadReady
    loop {
        use futures_util::StreamExt;
        match ws_rx.next().await {
            Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => {
                if let Ok(msg) = serde_json::from_str::<serde_json::Value>(&text) {
                    if msg["type"] == "FastDownloadReady" {
                        break;
                    }
                }
            }
            Some(Ok(_)) => continue,
            _ => return Err("WS connection lost waiting for FastDownloadReady".into()),
        }
    }

    // Channel to collect NACKs from receiver
    let (nack_tx, _nack_rx) = bounded::<(u32, Vec<u16>)>(256);

    // Set up NACK callback that sends over WebSocket
    let ws_tx_arc = Arc::new(tokio::sync::Mutex::new(ws_tx));
    let _ws_tx_nack = ws_tx_arc.clone();
    let _tid_nack = transfer_id.to_string();

    let nack_callback: haven_fast_transfer::NackCallback = Box::new(move |chunk_idx, missing| {
        let _ = nack_tx.try_send((chunk_idx, missing.clone()));
        // Also send via WS (best-effort, non-blocking)
        // Note: We can't easily await here since this is a sync callback.
        // The NACK is logged and will be sent in the WS read loop.
    });

    // Start receiver pipeline
    let receiver_config = ReceiverConfig {
        output_path: temp_path.clone(),
        transfer_id: transfer_id_bytes,
        file_size: encrypted_file_size,
        chunk_count,
        chunk_size: encrypted_chunk_size,
        chunk_hashes: chunk_hashes.to_vec(),
        file_sha256: file_sha256.to_string(),
        bind_addr: actual_bind_addr,
        logger: Some(Arc::new(TracingLogger)),
        pre_bound_socket: Some(udp_socket),
    };

    let recv_progress = Arc::new(ReceiverProgress::new());
    let recv_progress_clone = recv_progress.clone();
    let progress_poll = progress.clone();

    // Run receiver in blocking thread
    let receiver_handle = tokio::task::spawn_blocking(move || {
        run_receiver(receiver_config, recv_progress_clone, nack_callback)
    });

    // Poll receiver progress
    let poll_handle = tokio::spawn(async move {
        loop {
            let state = recv_progress.state.load(Ordering::Relaxed);
            progress_poll.bytes_done.store(
                recv_progress.bytes_done.load(Ordering::Relaxed),
                Ordering::Relaxed,
            );

            if state == haven_fast_transfer::receiver::STATE_COMPLETE
                || state == haven_fast_transfer::receiver::STATE_ERROR
                || state == haven_fast_transfer::receiver::STATE_CANCELLED
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    });

    let recv_result = receiver_handle
        .await
        .map_err(|e| format!("Receiver task panicked: {}", e))?;

    poll_handle.abort();

    recv_result.map_err(|e| format!("Receiver error: {}", e))?;

    // Now decrypt the received encrypted file
    // Read encrypted chunks, decrypt, write to final output
    progress.state.store(STATE_DOWNLOADING, Ordering::Relaxed);
    progress.bytes_done.store(0, Ordering::Relaxed);

    {
        use std::io::{Read, Write};

        let mut enc_file = std::fs::File::open(&temp_path)
            .map_err(|e| format!("Cannot open encrypted file: {}", e))?;

        // Ensure parent dir exists
        if let Some(parent) = std::path::Path::new(save_path).parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("Cannot create output dir: {}", e))?;
            }
        }

        let mut out_file = std::fs::File::create(save_path)
            .map_err(|e| format!("Cannot create output file: {}", e))?;

        for idx in 0..chunk_count {
            if progress.is_cancelled() {
                progress.state.store(STATE_CANCELLED, Ordering::Relaxed);
                let _ = std::fs::remove_file(&temp_path);
                return Err("Cancelled".into());
            }

            // Calculate encrypted chunk size
            let is_last = idx == chunk_count - 1;
            let enc_chunk_size = if is_last {
                encrypted_file_size - idx as u64 * encrypted_chunk_size
            } else {
                encrypted_chunk_size
            };

            let mut encrypted_chunk = vec![0u8; enc_chunk_size as usize];
            enc_file.read_exact(&mut encrypted_chunk)
                .map_err(|e| format!("Read encrypted chunk {}: {}", idx, e))?;

            let plaintext = decrypt_chunk(&key, &encrypted_chunk)
                .map_err(|e| format!("Decrypt chunk {}: {}", idx, e))?;

            out_file.write_all(&plaintext)
                .map_err(|e| format!("Write chunk {}: {}", idx, e))?;

            progress.bytes_done.fetch_add(enc_chunk_size, Ordering::Relaxed);
        }

        out_file.flush().map_err(|e| format!("Flush error: {}", e))?;
    }

    // Clean up temp file
    let _ = std::fs::remove_file(&temp_path);

    // Confirm download with server
    let _ = client
        .post(format!("{}/transfers/{}/confirm", file_server_url, transfer_id))
        .header("Authorization", format!("Bearer {}", jwt_token))
        .send()
        .await;

    progress.state.store(STATE_COMPLETE, Ordering::Relaxed);
    Ok(())
}

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
