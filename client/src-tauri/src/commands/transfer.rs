//! Native file transfer engine — reads files via tokio::fs, encrypts with
//! AES-256-GCM, and sends/receives over a persistent TCP connection to the
//! server's TCP relay.  Bypasses the WebView2/browser bottleneck entirely.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use bytes::Bytes;
use dashmap::DashMap;
use hkdf::Hkdf;
use sha2::Sha256;
use tauri::{AppHandle, Emitter, State};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, Mutex, Notify};
use tokio::task::JoinHandle;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// 1 MB chunks (4x the old 256 KB WebSocket chunks).
const CHUNK_SIZE: usize = 1024 * 1024;

/// Number of unacked chunks the sender may have in-flight.
const SEND_WINDOW: u32 = 32;

/// Receiver sends an ACK every this many chunks.
const ACK_INTERVAL: u32 = 8;

/// 4 MB socket buffers.
const SOCKET_BUF_SIZE: usize = 4 * 1024 * 1024;

// Frame types — match the TCP relay protocol
const FRAME_FILE_CHUNK: u8 = 0x10;
const FRAME_FILE_ACK: u8 = 0x11;
const FRAME_FILE_DONE: u8 = 0x12;
const FRAME_FILE_CANCEL: u8 = 0x13;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Progress payload emitted to the frontend via Tauri events.
#[derive(serde::Serialize, Clone)]
pub struct TransferProgress {
    pub transfer_id: String,
    pub bytes_sent: u64,
    pub total_bytes: u64,
    pub speed_mbps: f64,
}

/// Per-send ACK tracking so the sender can implement window-based flow control.
struct AckState {
    last_acked: AtomicI64,
    notify: Notify,
}

/// Frames delivered from the reader task to per-transfer receive tasks.
enum IncomingFrame {
    Chunk { chunk_index: u32, payload: Bytes },
    Done,
    Cancel,
}

// ---------------------------------------------------------------------------
// Managed State
// ---------------------------------------------------------------------------

/// Tauri managed state — one instance shared across all commands.
pub struct TransferEngine {
    /// TCP write half — shared by all outgoing frame writes.
    tcp_write: Arc<Mutex<Option<tokio::net::tcp::OwnedWriteHalf>>>,
    /// Background reader task handle.
    reader_handle: Mutex<Option<JoinHandle<()>>>,
    /// Pending receive registrations: transfer_id -> frame sender.
    pending_receives: Arc<DashMap<Uuid, mpsc::Sender<IncomingFrame>>>,
    /// ACK state for active sends: transfer_id -> AckState.
    send_acks: Arc<DashMap<Uuid, Arc<AckState>>>,
    /// Handles for active send tasks so we can abort on cancel.
    active_sends: Arc<DashMap<Uuid, JoinHandle<()>>>,
}

impl Default for TransferEngine {
    fn default() -> Self {
        Self {
            tcp_write: Arc::new(Mutex::new(None)),
            reader_handle: Mutex::new(None),
            pending_receives: Arc::new(DashMap::new()),
            send_acks: Arc::new(DashMap::new()),
            active_sends: Arc::new(DashMap::new()),
        }
    }
}

// ---------------------------------------------------------------------------
// Crypto helpers — must produce identical ciphertext to the JS implementation
// ---------------------------------------------------------------------------

/// Derive a 32-byte AES-256 key from the shared channel key using HKDF-SHA256.
///
/// Matches the JS Web Crypto derivation exactly:
///   salt  = 32 zero bytes
///   info  = UTF-8("haven-file-{transfer_id}")
///   hash  = SHA-256
fn derive_aes_key(channel_key_b64: &str, transfer_id: &str) -> Result<[u8; 32], String> {
    let ikm = base64_decode(channel_key_b64).map_err(|e| e.to_string())?;
    let hk = Hkdf::<Sha256>::new(Some(&[0u8; 32]), &ikm);
    let info = format!("haven-file-{}", transfer_id);
    let mut okm = [0u8; 32];
    hk.expand(info.as_bytes(), &mut okm)
        .map_err(|e| format!("HKDF expand failed: {}", e))?;
    Ok(okm)
}

/// Build the 12-byte AES-GCM nonce for a given chunk index.
///
/// Layout (matches JS `makeNonce`):
///   bytes 0..4  = 0x00
///   bytes 4..8  = (chunk_index >> 32) as u32 BE  (always 0 for u32 indices)
///   bytes 8..12 = chunk_index as u32 BE
fn make_nonce(chunk_index: u32) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    nonce[8..12].copy_from_slice(&chunk_index.to_be_bytes());
    nonce
}

fn encrypt_chunk(
    cipher: &Aes256Gcm,
    plaintext: &[u8],
    chunk_index: u32,
) -> Result<Vec<u8>, String> {
    let nonce_bytes = make_nonce(chunk_index);
    let nonce = Nonce::from_slice(&nonce_bytes);
    cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| format!("Encrypt failed: {}", e))
}

fn decrypt_chunk(
    cipher: &Aes256Gcm,
    ciphertext: &[u8],
    chunk_index: u32,
) -> Result<Vec<u8>, String> {
    let nonce_bytes = make_nonce(chunk_index);
    let nonce = Nonce::from_slice(&nonce_bytes);
    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| format!("Decrypt failed: {}", e))
}

fn base64_decode(data: &str) -> Result<Vec<u8>, base64::DecodeError> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(data)
}

// ---------------------------------------------------------------------------
// TCP I/O helpers
// ---------------------------------------------------------------------------

/// Write a length-prefixed frame to the shared TCP connection.
async fn write_frame(
    writer: &Arc<Mutex<Option<tokio::net::tcp::OwnedWriteHalf>>>,
    frame: &[u8],
) -> Result<(), String> {
    let mut guard = writer.lock().await;
    let w = guard.as_mut().ok_or("TCP relay not connected")?;
    let len = frame.len() as u32;
    w.write_all(&len.to_be_bytes())
        .await
        .map_err(|e| e.to_string())?;
    w.write_all(frame).await.map_err(|e| e.to_string())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Connect to the server's TCP relay, authenticate, and spawn the reader task.
#[tauri::command]
pub async fn transfer_connect(
    server_host: String,
    relay_port: u16,
    jwt_token: String,
    engine: State<'_, TransferEngine>,
    app: AppHandle,
) -> Result<(), String> {
    // Tear down any existing connection
    {
        let mut handle = engine.reader_handle.lock().await;
        if let Some(h) = handle.take() {
            h.abort();
        }
    }
    {
        let mut w = engine.tcp_write.lock().await;
        *w = None;
    }

    // Connect
    let addr = format!("{}:{}", server_host, relay_port);
    let stream = tokio::net::TcpStream::connect(&addr)
        .await
        .map_err(|e| format!("TCP connect to {} failed: {}", addr, e))?;

    // Socket options for throughput
    let sock_ref = socket2::SockRef::from(&stream);
    sock_ref.set_nodelay(true).map_err(|e| e.to_string())?;
    let _ = sock_ref.set_send_buffer_size(SOCKET_BUF_SIZE);
    let _ = sock_ref.set_recv_buffer_size(SOCKET_BUF_SIZE);

    let (mut reader, mut writer) = stream.into_split();

    // Auth handshake: [0x00][jwt_len(2 BE)][jwt_bytes]
    let jwt_bytes = jwt_token.as_bytes();
    let mut auth_frame = Vec::with_capacity(3 + jwt_bytes.len());
    auth_frame.push(0x00);
    auth_frame.extend_from_slice(&(jwt_bytes.len() as u16).to_be_bytes());
    auth_frame.extend_from_slice(jwt_bytes);
    writer
        .write_all(&auth_frame)
        .await
        .map_err(|e| e.to_string())?;

    // Read response: [0x00][0x01] = OK, [0x00][0x00] = reject
    let mut resp = [0u8; 2];
    reader
        .read_exact(&mut resp)
        .await
        .map_err(|e| e.to_string())?;
    if resp != [0x00, 0x01] {
        return Err("TCP relay auth rejected".into());
    }

    // Store write half
    {
        let mut w = engine.tcp_write.lock().await;
        *w = Some(writer);
    }

    // Spawn reader task
    let pending_receives = engine.pending_receives.clone();
    let send_acks = engine.send_acks.clone();
    let reader_handle = tokio::spawn(async move {
        if let Err(e) = reader_loop(reader, pending_receives, send_acks, app).await {
            eprintln!("[transfer] Reader loop error: {}", e);
        }
    });

    {
        let mut handle = engine.reader_handle.lock().await;
        *handle = Some(reader_handle);
    }

    Ok(())
}

/// Background task that reads frames from the TCP connection and routes them
/// to the appropriate send/receive handler.
async fn reader_loop(
    mut reader: tokio::net::tcp::OwnedReadHalf,
    pending_receives: Arc<DashMap<Uuid, mpsc::Sender<IncomingFrame>>>,
    send_acks: Arc<DashMap<Uuid, Arc<AckState>>>,
    app: AppHandle,
) -> Result<(), String> {
    loop {
        let frame_len = match reader.read_u32().await {
            Ok(len) => len as usize,
            Err(_) => {
                let _ = app.emit("transfer-relay-disconnected", ());
                return Ok(());
            }
        };

        if frame_len == 0 || frame_len > 16 * 1024 * 1024 {
            return Err(format!("Invalid frame length: {}", frame_len));
        }

        let mut frame = vec![0u8; frame_len];
        reader
            .read_exact(&mut frame)
            .await
            .map_err(|e| e.to_string())?;

        if frame.is_empty() {
            continue;
        }

        let msg_type = frame[0];

        match msg_type {
            // FileChunk: [type(1)][from_uid(16)][transfer_id(16)][chunk_idx(4)][payload...]
            FRAME_FILE_CHUNK => {
                if frame.len() < 37 {
                    continue;
                }
                let transfer_id =
                    Uuid::from_bytes(frame[17..33].try_into().unwrap());
                let chunk_index =
                    u32::from_be_bytes(frame[33..37].try_into().unwrap());
                let payload = Bytes::copy_from_slice(&frame[37..]);

                if let Some(tx) = pending_receives.get(&transfer_id) {
                    let _ = tx.try_send(IncomingFrame::Chunk {
                        chunk_index,
                        payload,
                    });
                }
            }
            // FileAck: [type(1)][from_uid(16)][transfer_id(16)][chunk_idx(4)]
            FRAME_FILE_ACK => {
                if frame.len() < 37 {
                    continue;
                }
                let transfer_id =
                    Uuid::from_bytes(frame[17..33].try_into().unwrap());
                let ack_index =
                    u32::from_be_bytes(frame[33..37].try_into().unwrap());

                if let Some(ack) = send_acks.get(&transfer_id) {
                    ack.last_acked.store(ack_index as i64, Ordering::Release);
                    ack.notify.notify_waiters();
                }
            }
            // FileDone: [type(1)][from_uid(16)][transfer_id(16)]
            FRAME_FILE_DONE => {
                if frame.len() < 33 {
                    continue;
                }
                let transfer_id =
                    Uuid::from_bytes(frame[17..33].try_into().unwrap());

                if let Some(tx) = pending_receives.get(&transfer_id) {
                    let _ = tx.try_send(IncomingFrame::Done);
                }
            }
            // FileCancel: [type(1)][from_uid(16)][transfer_id(16)]
            FRAME_FILE_CANCEL => {
                if frame.len() < 33 {
                    continue;
                }
                let transfer_id =
                    Uuid::from_bytes(frame[17..33].try_into().unwrap());

                if let Some(tx) = pending_receives.get(&transfer_id) {
                    let _ = tx.try_send(IncomingFrame::Cancel);
                }
                // Also unblock any waiting sender
                if let Some(ack) = send_acks.get(&transfer_id) {
                    ack.last_acked.store(i64::MAX, Ordering::Release);
                    ack.notify.notify_waiters();
                }
            }
            _ => {}
        }
    }
}

/// Start sending a file over the native TCP relay.
///
/// Reads the file via tokio::fs, encrypts each 1 MB chunk with AES-256-GCM,
/// and sends length-prefixed frames over the persistent TCP connection.
/// Emits `transfer-progress` events to the frontend.
#[tauri::command]
pub async fn transfer_send_file(
    app: AppHandle,
    file_path: String,
    target_uid: String,
    transfer_id: String,
    channel_key: String,
    engine: State<'_, TransferEngine>,
) -> Result<(), String> {
    let target_uuid =
        Uuid::parse_str(&target_uid).map_err(|e| e.to_string())?;
    let transfer_uuid =
        Uuid::parse_str(&transfer_id).map_err(|e| e.to_string())?;

    // Derive AES key (must match JS HKDF derivation)
    let aes_key = derive_aes_key(&channel_key, &transfer_id)?;
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&aes_key));

    // File metadata
    let metadata = tokio::fs::metadata(&file_path)
        .await
        .map_err(|e| e.to_string())?;
    let total_size = metadata.len();

    // Set up ACK tracking
    let ack_state = Arc::new(AckState {
        last_acked: AtomicI64::new(-1),
        notify: Notify::new(),
    });
    engine
        .send_acks
        .insert(transfer_uuid, ack_state.clone());

    let tcp_write = engine.tcp_write.clone();
    let send_acks = engine.send_acks.clone();
    let active_sends = engine.active_sends.clone();

    let handle = tokio::spawn(async move {
        let result = send_file_task(
            app.clone(),
            &file_path,
            target_uuid,
            transfer_uuid,
            cipher,
            total_size,
            tcp_write,
            ack_state,
        )
        .await;

        // Cleanup
        send_acks.remove(&transfer_uuid);
        active_sends.remove(&transfer_uuid);

        if let Err(e) = result {
            eprintln!("[transfer] Send error: {}", e);
            let _ = app.emit(
                "transfer-error",
                serde_json::json!({
                    "transfer_id": transfer_uuid.to_string(),
                    "error": e,
                }),
            );
        }
    });

    engine.active_sends.insert(transfer_uuid, handle);
    Ok(())
}

/// Inner send loop — reads file, encrypts, sends frames with flow control.
async fn send_file_task(
    app: AppHandle,
    file_path: &str,
    target_uid: Uuid,
    transfer_id: Uuid,
    cipher: Aes256Gcm,
    total_size: u64,
    tcp_write: Arc<Mutex<Option<tokio::net::tcp::OwnedWriteHalf>>>,
    ack_state: Arc<AckState>,
) -> Result<(), String> {
    let mut file = tokio::fs::File::open(file_path)
        .await
        .map_err(|e| e.to_string())?;

    let mut chunk_index: u32 = 0;
    let mut bytes_sent: u64 = 0;
    let start_time = std::time::Instant::now();
    let mut last_progress = std::time::Instant::now();
    let mut buf = vec![0u8; CHUNK_SIZE];

    loop {
        // Read a full chunk (may need multiple reads for large chunks)
        let mut total_read = 0;
        while total_read < CHUNK_SIZE {
            let n = file
                .read(&mut buf[total_read..])
                .await
                .map_err(|e| e.to_string())?;
            if n == 0 {
                break; // EOF
            }
            total_read += n;
        }

        if total_read == 0 {
            break; // Done reading the file
        }

        let plaintext = &buf[..total_read];

        // Encrypt chunk
        let ciphertext = encrypt_chunk(&cipher, plaintext, chunk_index)?;

        // Flow control: wait until we have window space
        loop {
            let acked = ack_state.last_acked.load(Ordering::Acquire);
            if acked == i64::MAX {
                // Transfer was cancelled by peer
                return Err("Transfer cancelled by peer".into());
            }
            let in_flight = chunk_index as i64 - acked;
            if in_flight < SEND_WINDOW as i64 {
                break;
            }
            ack_state.notify.notified().await;
        }

        // Build frame: [type(1)][target_uid(16)][transfer_id(16)][chunk_idx(4)][payload...]
        let frame_len = 37 + ciphertext.len();
        let mut frame = Vec::with_capacity(frame_len);
        frame.push(FRAME_FILE_CHUNK);
        frame.extend_from_slice(target_uid.as_bytes());
        frame.extend_from_slice(transfer_id.as_bytes());
        frame.extend_from_slice(&chunk_index.to_be_bytes());
        frame.extend_from_slice(&ciphertext);

        write_frame(&tcp_write, &frame).await?;

        bytes_sent += total_read as u64;
        chunk_index += 1;

        // Emit progress (~10 fps)
        if last_progress.elapsed().as_millis() >= 100 {
            let elapsed = start_time.elapsed().as_secs_f64();
            let speed = if elapsed > 0.0 {
                bytes_sent as f64 / elapsed / 1_000_000.0
            } else {
                0.0
            };
            let _ = app.emit(
                "transfer-progress",
                TransferProgress {
                    transfer_id: transfer_id.to_string(),
                    bytes_sent,
                    total_bytes: total_size,
                    speed_mbps: speed,
                },
            );
            last_progress = std::time::Instant::now();
        }
    }

    // Send FileDone frame
    let mut done_frame = Vec::with_capacity(33);
    done_frame.push(FRAME_FILE_DONE);
    done_frame.extend_from_slice(target_uid.as_bytes());
    done_frame.extend_from_slice(transfer_id.as_bytes());
    write_frame(&tcp_write, &done_frame).await?;

    // Final progress event
    let elapsed = start_time.elapsed().as_secs_f64();
    let speed = if elapsed > 0.0 {
        bytes_sent as f64 / elapsed / 1_000_000.0
    } else {
        0.0
    };
    let _ = app.emit(
        "transfer-progress",
        TransferProgress {
            transfer_id: transfer_id.to_string(),
            bytes_sent,
            total_bytes: total_size,
            speed_mbps: speed,
        },
    );
    let _ = app.emit(
        "transfer-complete",
        serde_json::json!({ "transfer_id": transfer_id.to_string() }),
    );

    Ok(())
}

/// Register a pending receive and spawn a task that writes incoming chunks to disk.
///
/// The reader task (spawned by `transfer_connect`) routes matching frames to
/// the mpsc channel. This task decrypts and writes them sequentially.
#[tauri::command]
pub async fn transfer_receive_file(
    app: AppHandle,
    save_path: String,
    transfer_id: String,
    total_size: u64,
    channel_key: String,
    peer_uid: String,
    engine: State<'_, TransferEngine>,
) -> Result<(), String> {
    let transfer_uuid =
        Uuid::parse_str(&transfer_id).map_err(|e| e.to_string())?;
    let peer_uuid =
        Uuid::parse_str(&peer_uid).map_err(|e| e.to_string())?;

    // Derive AES key
    let aes_key = derive_aes_key(&channel_key, &transfer_id)?;
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&aes_key));

    // Create channel for incoming frames from the reader task
    let (tx, mut rx) = mpsc::channel::<IncomingFrame>(256);
    engine.pending_receives.insert(transfer_uuid, tx);

    let pending_receives = engine.pending_receives.clone();
    let tcp_write = engine.tcp_write.clone();

    tokio::spawn(async move {
        let result = receive_file_task(
            app.clone(),
            &save_path,
            transfer_uuid,
            total_size,
            cipher,
            &mut rx,
            tcp_write,
            peer_uuid,
        )
        .await;

        pending_receives.remove(&transfer_uuid);

        if let Err(e) = result {
            eprintln!("[transfer] Receive error: {}", e);
            let _ = app.emit(
                "transfer-error",
                serde_json::json!({
                    "transfer_id": transfer_uuid.to_string(),
                    "error": e,
                }),
            );
        }
    });

    Ok(())
}

/// Inner receive loop — decrypts chunks and writes to disk.
async fn receive_file_task(
    app: AppHandle,
    save_path: &str,
    transfer_id: Uuid,
    total_size: u64,
    cipher: Aes256Gcm,
    rx: &mut mpsc::Receiver<IncomingFrame>,
    tcp_write: Arc<Mutex<Option<tokio::net::tcp::OwnedWriteHalf>>>,
    peer_uid: Uuid,
) -> Result<(), String> {
    let mut file = tokio::fs::File::create(save_path)
        .await
        .map_err(|e| e.to_string())?;

    let mut bytes_received: u64 = 0;
    let start_time = std::time::Instant::now();
    let mut last_progress = std::time::Instant::now();

    while let Some(frame) = rx.recv().await {
        match frame {
            IncomingFrame::Chunk {
                chunk_index,
                payload,
            } => {
                // Decrypt
                let plaintext =
                    decrypt_chunk(&cipher, &payload, chunk_index)?;

                // Write to file
                file.write_all(&plaintext)
                    .await
                    .map_err(|e| e.to_string())?;

                bytes_received += plaintext.len() as u64;

                // Send ACK every ACK_INTERVAL chunks
                if chunk_index > 0 && chunk_index % ACK_INTERVAL == 0 {
                    let mut ack_frame = Vec::with_capacity(37);
                    ack_frame.push(FRAME_FILE_ACK);
                    ack_frame.extend_from_slice(peer_uid.as_bytes());
                    ack_frame.extend_from_slice(transfer_id.as_bytes());
                    ack_frame
                        .extend_from_slice(&chunk_index.to_be_bytes());
                    let _ = write_frame(&tcp_write, &ack_frame).await;
                }

                // Emit progress (~10 fps)
                if last_progress.elapsed().as_millis() >= 100 {
                    let elapsed = start_time.elapsed().as_secs_f64();
                    let speed = if elapsed > 0.0 {
                        bytes_received as f64 / elapsed / 1_000_000.0
                    } else {
                        0.0
                    };
                    let _ = app.emit(
                        "transfer-progress",
                        TransferProgress {
                            transfer_id: transfer_id.to_string(),
                            bytes_sent: bytes_received,
                            total_bytes: total_size,
                            speed_mbps: speed,
                        },
                    );
                    last_progress = std::time::Instant::now();
                }
            }
            IncomingFrame::Done => {
                // Flush to disk
                file.flush().await.map_err(|e| e.to_string())?;

                // Send final ACK (sentinel 0xFFFFFFFF)
                let mut ack_frame = Vec::with_capacity(37);
                ack_frame.push(FRAME_FILE_ACK);
                ack_frame.extend_from_slice(peer_uid.as_bytes());
                ack_frame.extend_from_slice(transfer_id.as_bytes());
                ack_frame
                    .extend_from_slice(&0xFFFFFFFFu32.to_be_bytes());
                let _ = write_frame(&tcp_write, &ack_frame).await;

                // Final progress + complete event
                let elapsed = start_time.elapsed().as_secs_f64();
                let speed = if elapsed > 0.0 {
                    bytes_received as f64 / elapsed / 1_000_000.0
                } else {
                    0.0
                };
                let _ = app.emit(
                    "transfer-progress",
                    TransferProgress {
                        transfer_id: transfer_id.to_string(),
                        bytes_sent: bytes_received,
                        total_bytes: total_size,
                        speed_mbps: speed,
                    },
                );
                let _ = app.emit(
                    "transfer-complete",
                    serde_json::json!({
                        "transfer_id": transfer_id.to_string(),
                    }),
                );
                return Ok(());
            }
            IncomingFrame::Cancel => {
                let _ = app.emit(
                    "transfer-cancelled",
                    serde_json::json!({
                        "transfer_id": transfer_id.to_string(),
                    }),
                );
                return Ok(());
            }
        }
    }

    Err("Receive channel closed unexpectedly".into())
}

/// Cancel an in-progress native transfer. Aborts send tasks, removes receive
/// registrations, and optionally sends a cancel frame to the peer.
#[tauri::command]
pub async fn transfer_cancel(
    transfer_id: String,
    target_uid: Option<String>,
    engine: State<'_, TransferEngine>,
) -> Result<(), String> {
    let transfer_uuid =
        Uuid::parse_str(&transfer_id).map_err(|e| e.to_string())?;

    // Remove from pending receives
    engine.pending_receives.remove(&transfer_uuid);

    // Abort any active send task
    if let Some((_, handle)) = engine.active_sends.remove(&transfer_uuid) {
        handle.abort();
    }

    // Remove ACK state
    engine.send_acks.remove(&transfer_uuid);

    // Send cancel frame to peer if target_uid is provided
    if let Some(target) = target_uid {
        if let Ok(target_uuid) = Uuid::parse_str(&target) {
            let mut cancel_frame = Vec::with_capacity(33);
            cancel_frame.push(FRAME_FILE_CANCEL);
            cancel_frame.extend_from_slice(target_uuid.as_bytes());
            cancel_frame.extend_from_slice(transfer_uuid.as_bytes());
            let _ = write_frame(&engine.tcp_write, &cancel_frame).await;
        }
    }

    Ok(())
}
