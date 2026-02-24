use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};

use reqwest::Client;
use sha2::{Sha256, Digest};
use tokio::io::AsyncReadExt;

use crate::crypto::{derive_key, derive_chunk_nonce, encrypt_chunk_with_nonce};

/// Transfer state constants.
pub const STATE_IDLE: u8 = 0;
pub const STATE_HASHING: u8 = 1;
pub const STATE_UPLOADING: u8 = 2;
pub const STATE_COMPLETE: u8 = 3;
pub const STATE_ERROR: u8 = 4;
pub const STATE_CANCELLED: u8 = 5;

const CHUNK_SIZE: usize = 4 * 1024 * 1024; // 4 MB

/// Shared progress state for FFI polling.
pub struct UploadProgress {
    pub bytes_done: AtomicU64,
    pub bytes_total: AtomicU64,
    pub state: AtomicU8,
    pub cancelled: AtomicU8,
    /// Set after pass 1 completes: JSON string `{"file_sha256":"...","chunk_hashes":[...]}`.
    pub hashes_json: std::sync::Mutex<Option<String>>,
}

impl UploadProgress {
    pub fn new() -> Self {
        Self {
            bytes_done: AtomicU64::new(0),
            bytes_total: AtomicU64::new(0),
            state: AtomicU8::new(STATE_IDLE),
            cancelled: AtomicU8::new(0),
            hashes_json: std::sync::Mutex::new(None),
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed) != 0
    }
}

/// Upload a file to the Haven file server.
///
/// Nonces are derived deterministically from (key, chunk_index) so that both
/// passes produce identical ciphertext — allowing per-chunk hash pre-computation
/// without buffering the entire encrypted file in RAM.
///
/// Pass 1: read + encrypt + hash each chunk, discard encrypted bytes.
/// Pass 2: POST /transfers with metadata, then re-read + encrypt + stream.
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
    let client = Client::new();

    let path = Path::new(file_path);
    let file_size = tokio::fs::metadata(path)
        .await
        .map_err(|e| format!("Cannot read file: {}", e))?
        .len();

    progress.bytes_total.store(file_size, Ordering::Relaxed);
    progress.state.store(STATE_HASHING, Ordering::Relaxed);

    // ── Pass 1: compute hashes without buffering encrypted data ─────────
    let mut chunk_hashes: Vec<String> = Vec::new();
    let mut full_hasher = Sha256::new();
    let mut encrypted_size: u64 = 0;
    let mut chunk_index: u64 = 0;

    {
        let mut file = tokio::fs::File::open(path)
            .await
            .map_err(|e| format!("Cannot open file for hashing: {}", e))?;
        let mut buf = vec![0u8; CHUNK_SIZE];

        loop {
            if progress.is_cancelled() {
                progress.state.store(STATE_CANCELLED, Ordering::Relaxed);
                return Err("Cancelled".into());
            }

            let n = read_chunk(&mut file, &mut buf).await?;
            if n == 0 {
                break;
            }

            let nonce = derive_chunk_nonce(&key, chunk_index);
            let encrypted = encrypt_chunk_with_nonce(&key, &buf[..n], nonce)?;

            let mut chunk_hasher = Sha256::new();
            chunk_hasher.update(&encrypted);
            chunk_hashes.push(hex::encode(chunk_hasher.finalize()));

            full_hasher.update(&encrypted);
            encrypted_size += encrypted.len() as u64;
            // `encrypted` dropped here — no RAM accumulation.

            chunk_index += 1;
        }
    }

    let file_sha256 = hex::encode(full_hasher.finalize());

    // Store hashes so Dart can read them via FFI and send the offer to the receiver.
    {
        let json = serde_json::json!({
            "file_sha256": file_sha256,
            "chunk_hashes": chunk_hashes,
        })
        .to_string();
        *progress.hashes_json.lock().unwrap() = Some(json);
    }

    // ── Create transfer on server ────────────────────────────────────────
    // Update bytes_total to encrypted_size so pass-2 progress is accurate
    // (bytes_done counts encrypted bytes, so total must match).
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

    let resp = client
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

    // ── Pass 2: re-read + encrypt on the fly (same deterministic nonces) ─
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|e| format!("Cannot open file for upload: {}", e))?;

    let progress_clone = progress.clone();

    let stream = futures_util::stream::unfold(
        (file, vec![0u8; CHUNK_SIZE], progress_clone, 0u64),
        move |(mut file, mut buf, prog, idx)| async move {
            if prog.is_cancelled() {
                prog.state.store(STATE_CANCELLED, Ordering::Relaxed);
                return None;
            }

            let n = match read_chunk(&mut file, &mut buf).await {
                Ok(0) => return None,
                Ok(n) => n,
                Err(e) => {
                    eprintln!("Read error during upload: {}", e);
                    return None;
                }
            };

            let nonce = derive_chunk_nonce(&key, idx);
            match encrypt_chunk_with_nonce(&key, &buf[..n], nonce) {
                Ok(encrypted) => {
                    prog.bytes_done
                        .fetch_add(encrypted.len() as u64, Ordering::Relaxed);
                    Some((
                        Ok::<_, std::io::Error>(bytes::Bytes::from(encrypted)),
                        (file, buf, prog, idx + 1),
                    ))
                }
                Err(e) => {
                    eprintln!("Encryption error during upload: {}", e);
                    None
                }
            }
        },
    );

    let body = reqwest::Body::wrap_stream(stream);

    let resp = client
        .put(format!("{}/transfers/{}/data", server_url, transfer_id))
        .header("Authorization", format!("Bearer {}", jwt_token))
        .header("Content-Type", "application/octet-stream")
        .header("Content-Length", encrypted_size.to_string())
        .body(body)
        .send()
        .await
        .map_err(|e| format!("Upload failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        progress.state.store(STATE_ERROR, Ordering::Relaxed);
        return Err(format!("Upload failed ({}): {}", status, body));
    }

    progress.state.store(STATE_COMPLETE, Ordering::Relaxed);
    Ok(())
}

async fn read_chunk(
    file: &mut tokio::fs::File,
    buf: &mut [u8],
) -> Result<usize, String> {
    let mut total = 0;
    while total < buf.len() {
        match file.read(&mut buf[total..]).await {
            Ok(0) => break,
            Ok(n) => total += n,
            Err(e) => return Err(format!("Read error: {}", e)),
        }
    }
    Ok(total)
}
