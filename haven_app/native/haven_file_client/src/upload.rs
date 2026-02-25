use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};

use reqwest::Client;
use sha2::{Sha256, Digest};
use tokio::io::AsyncReadExt;
use tokio::sync::Semaphore;

use crate::crypto::{derive_key, derive_chunk_nonce, encrypt_chunk_with_nonce};

/// Transfer state constants.
pub const STATE_IDLE: u8 = 0;
pub const STATE_HASHING: u8 = 1;
pub const STATE_UPLOADING: u8 = 2;
pub const STATE_COMPLETE: u8 = 3;
pub const STATE_ERROR: u8 = 4;
pub const STATE_CANCELLED: u8 = 5;

const CHUNK_SIZE: usize = 4 * 1024 * 1024; // 4 MB

/// Max number of chunks in flight (encrypt + upload) simultaneously.
/// On a gigabit LAN: 4 MB / 125 MB/s ≈ 32 ms per chunk.
/// 8 in flight keeps the pipe full while the disk reads the next chunk.
const UPLOAD_CONCURRENCY: usize = 8;

/// Shared progress state for FFI polling.
pub struct UploadProgress {
    pub bytes_done: AtomicU64,
    pub bytes_total: AtomicU64,
    pub state: AtomicU8,
    pub cancelled: AtomicU8,
    /// Set after pass 1 completes: JSON string `{"file_sha256":"...","chunk_hashes":[...]}`.
    pub hashes_json: std::sync::Mutex<Option<String>>,
    /// Last error message, readable from FFI after STATE_ERROR.
    pub last_error: std::sync::Mutex<Option<String>>,
}

impl UploadProgress {
    pub fn new() -> Self {
        Self {
            bytes_done: AtomicU64::new(0),
            bytes_total: AtomicU64::new(0),
            state: AtomicU8::new(STATE_IDLE),
            cancelled: AtomicU8::new(0),
            hashes_json: std::sync::Mutex::new(None),
            last_error: std::sync::Mutex::new(None),
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed) != 0
    }
}

/// Upload a file to the Haven file server.
///
/// Pass 1: single sequential read — compute per-chunk encrypted hashes and
///         full-file hash in one pass. No random seeks, no rayon parallelism.
///         Disk throughput on HDD is ~150 MB/s sequential vs ~1 MB/s random.
///
/// Pass 2: sequential async read (one file handle, forward-only) feeds chunks
///         to a bounded pool of tokio tasks. Each task encrypts on a blocking
///         thread (spawn_blocking) then PUTs to /transfers/{id}/chunks/{index}
///         via an async reqwest client. Semaphore limits concurrency to 8 so
///         the pipe stays full without overwhelming the server.
pub async fn upload_file(
    file_path: &str,
    server_url: &str,
    transfer_id: &str,
    jwt_token: &str,
    master_key: &[u8],
    salt: &[u8],
    progress: Arc<UploadProgress>,
) -> Result<(), String> {
    let key = derive_key(master_key, salt);
    let async_client = Client::new();

    let path = Path::new(file_path);
    let file_size = tokio::fs::metadata(path)
        .await
        .map_err(|e| format!("Cannot read file: {}", e))?
        .len();

    progress.bytes_total.store(file_size, Ordering::Relaxed);
    progress.state.store(STATE_HASHING, Ordering::Relaxed);

    let chunk_count = if file_size == 0 {
        1
    } else {
        (file_size as usize + CHUNK_SIZE - 1) / CHUNK_SIZE
    };

    // ── Pass 1: single sequential read, compute per-chunk and full-file hashes ─
    // One file open, forward-only reads — no seek contention on HDD.
    let (chunk_hashes, file_sha256, encrypted_size) = {
        let file_path_owned = file_path.to_string();
        let progress_p1 = progress.clone();

        tokio::task::block_in_place(|| -> Result<(Vec<String>, String, u64), String> {
            use std::io::Read;
            let mut file = std::fs::File::open(&file_path_owned)
                .map_err(|e| format!("Cannot open file: {}", e))?;

            let mut full_hasher = Sha256::new();
            let mut chunk_hashes = Vec::with_capacity(chunk_count);
            let mut encrypted_size: u64 = 0;
            let mut buf = vec![0u8; CHUNK_SIZE];

            for idx in 0..chunk_count {
                if progress_p1.is_cancelled() {
                    return Err("Cancelled".to_string());
                }

                let remaining = file_size - idx as u64 * CHUNK_SIZE as u64;
                let to_read = (remaining as usize).min(CHUNK_SIZE);

                file.read_exact(&mut buf[..to_read])
                    .map_err(|e| format!("Read error at chunk {}: {}", idx, e))?;

                let nonce = derive_chunk_nonce(&key, idx as u64);
                let encrypted = encrypt_chunk_with_nonce(&key, &buf[..to_read], nonce)?;

                let mut chunk_hasher = Sha256::new();
                chunk_hasher.update(&encrypted);
                chunk_hashes.push(hex::encode(chunk_hasher.finalize()));

                full_hasher.update(&encrypted);
                encrypted_size += encrypted.len() as u64;
            }

            let file_sha256 = hex::encode(full_hasher.finalize());
            Ok((chunk_hashes, file_sha256, encrypted_size))
        })?
    };

    if progress.is_cancelled() {
        progress.state.store(STATE_CANCELLED, Ordering::Relaxed);
        return Err("Cancelled".into());
    }

    // Store hashes so Dart can read them via FFI and send the offer to the receiver.
    {
        let json = serde_json::json!({
            "file_sha256": file_sha256,
            "chunk_hashes": chunk_hashes,
        })
        .to_string();
        *progress.hashes_json.lock().unwrap() = Some(json);
    }

    // ── Create transfer on server ─────────────────────────────────────────────
    progress.bytes_total.store(encrypted_size, Ordering::Relaxed);
    progress.bytes_done.store(0, Ordering::Relaxed);
    progress.state.store(STATE_UPLOADING, Ordering::Relaxed);

    let encrypted_chunk_size = CHUNK_SIZE + 28; // 12-byte nonce + 16-byte GCM tag

    let create_body = serde_json::json!({
        "id": transfer_id,
        "file_size": encrypted_size,
        "chunk_size": encrypted_chunk_size,
        "file_sha256": file_sha256,
        "chunk_hashes": chunk_hashes,
    });

    let resp = async_client
        .post(format!("{}/transfers", server_url))
        .header("Authorization", format!("Bearer {}", jwt_token))
        .json(&create_body)
        .send()
        .await
        .map_err(|e| format!("Create transfer failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Create transfer failed ({}): {}", status, body));
    }

    // ── Pass 2: sequential read → parallel encrypt + upload ──────────────────
    // Read chunks one at a time (forward, no seeks) and dispatch each to a
    // tokio task bounded by a semaphore. The task encrypts on a blocking thread
    // then uploads asynchronously. While the network is busy sending N chunks,
    // the disk is reading the next one — they overlap naturally.
    let semaphore = Arc::new(Semaphore::new(UPLOAD_CONCURRENCY));
    let mut handles = Vec::with_capacity(chunk_count);

    let mut file = tokio::fs::File::open(file_path)
        .await
        .map_err(|e| format!("Cannot open file for upload: {}", e))?;

    for idx in 0..chunk_count {
        if progress.is_cancelled() {
            progress.state.store(STATE_CANCELLED, Ordering::Relaxed);
            return Err("Cancelled".into());
        }

        let remaining = file_size - idx as u64 * CHUNK_SIZE as u64;
        let to_read = (remaining as usize).min(CHUNK_SIZE);
        let mut buf = vec![0u8; to_read];

        // Sequential read — single file handle, forward-only, no seek.
        file.read_exact(&mut buf)
            .await
            .map_err(|e| format!("Read error at chunk {}: {}", idx, e))?;

        // Acquire semaphore slot before spawning (backpressure: don't read
        // ahead unboundedly if uploads can't keep up).
        let permit = semaphore.clone().acquire_owned().await.unwrap();

        let key_copy = key; // [u8; 32] is Copy
        let server_url_clone = server_url.to_string();
        let transfer_id_clone = transfer_id.to_string();
        let jwt_clone = jwt_token.to_string();
        let progress_clone = progress.clone();
        let client_clone = async_client.clone();

        let handle = tokio::spawn(async move {
            let _permit = permit; // released when task completes

            // Encrypt on a blocking thread — don't stall the async executor.
            let nonce = derive_chunk_nonce(&key_copy, idx as u64);
            let encrypted = tokio::task::spawn_blocking(move || {
                encrypt_chunk_with_nonce(&key_copy, &buf, nonce)
            })
            .await
            .map_err(|e| format!("Encryption task panicked at chunk {}: {}", idx, e))??;

            let enc_len = encrypted.len() as u64;

            let url = format!(
                "{}/transfers/{}/chunks/{}",
                server_url_clone, transfer_id_clone, idx
            );

            let resp = client_clone
                .put(&url)
                .header("Authorization", format!("Bearer {}", jwt_clone))
                .header("Content-Type", "application/octet-stream")
                .body(encrypted)
                .send()
                .await
                .map_err(|e| format!("Chunk {} upload failed: {}", idx, e))?;

            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                return Err(format!(
                    "Chunk {} upload failed ({}): {}",
                    idx, status, body
                ));
            }

            progress_clone.bytes_done.fetch_add(enc_len, Ordering::Relaxed);
            Ok::<(), String>(())
        });

        handles.push(handle);
    }

    // Wait for all upload tasks to finish.
    for handle in handles {
        if progress.is_cancelled() {
            progress.state.store(STATE_CANCELLED, Ordering::Relaxed);
            return Err("Cancelled".into());
        }
        handle
            .await
            .map_err(|e| format!("Upload task panicked: {}", e))??;
    }

    if progress.is_cancelled() {
        progress.state.store(STATE_CANCELLED, Ordering::Relaxed);
        return Err("Cancelled".into());
    }

    progress.state.store(STATE_COMPLETE, Ordering::Relaxed);
    Ok(())
}
