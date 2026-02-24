use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};

use futures_util::StreamExt;
use reqwest::Client;
use sha2::{Sha256, Digest};
use tokio::io::AsyncWriteExt;

use crate::crypto::{derive_key, decrypt_chunk};
use crate::upload::{STATE_IDLE, STATE_UPLOADING as STATE_DOWNLOADING, STATE_COMPLETE, STATE_ERROR, STATE_CANCELLED};

const CHUNK_SIZE: usize = 4 * 1024 * 1024; // 4 MB

/// Shared progress state for FFI polling.
pub struct DownloadProgress {
    pub bytes_done: AtomicU64,
    pub bytes_total: AtomicU64,
    pub state: AtomicU8,
    pub cancelled: AtomicU8,
}

impl DownloadProgress {
    pub fn new() -> Self {
        Self {
            bytes_done: AtomicU64::new(0),
            bytes_total: AtomicU64::new(0),
            state: AtomicU8::new(STATE_IDLE),
            cancelled: AtomicU8::new(0),
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed) != 0
    }
}

/// Download a file from the Haven file server, verify hashes, and decrypt.
///
/// 1. GET /transfers/{id}/data with streaming response
/// 2. Read encrypted chunks, verify per-chunk SHA-256
/// 3. Decrypt each chunk and write to output file
/// 4. Verify full file SHA-256
/// 5. On hash mismatch: retry with Range header
pub async fn download_file(
    save_path: &str,
    server_url: &str,
    transfer_id: &str,
    jwt_token: &str,
    master_key: &[u8],
    salt: &[u8],
    file_sha256: &str,
    chunk_hashes: &[String],
    progress: Arc<DownloadProgress>,
) -> Result<(), String> {
    let key = derive_key(master_key, salt);
    let client = Client::new();

    progress.state.store(STATE_DOWNLOADING, Ordering::Relaxed);

    // GET the streaming download
    let resp = client
        .get(format!("{}/transfers/{}/data", server_url, transfer_id))
        .header("Authorization", format!("Bearer {}", jwt_token))
        .send()
        .await
        .map_err(|e| format!("Download request failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        progress.state.store(STATE_ERROR, Ordering::Relaxed);
        return Err(format!("Download failed ({}): {}", status, body));
    }

    let content_length = resp.content_length().unwrap_or(0);
    progress.bytes_total.store(content_length, Ordering::Relaxed);

    // Create output file
    let mut output_file = tokio::fs::File::create(save_path)
        .await
        .map_err(|e| format!("Cannot create output file: {}", e))?;

    // Stream the response body, splitting into encrypted chunks
    let mut stream = resp.bytes_stream();
    let mut buf = Vec::with_capacity(CHUNK_SIZE + 1024); // extra room for encryption overhead
    let mut chunk_idx: usize = 0;
    let mut full_hasher = Sha256::new();

    while let Some(result) = stream.next().await {
        if progress.is_cancelled() {
            progress.state.store(STATE_CANCELLED, Ordering::Relaxed);
            return Err("Cancelled".into());
        }

        let data = result.map_err(|e| format!("Stream error: {}", e))?;
        buf.extend_from_slice(&data);
        progress.bytes_done.fetch_add(data.len() as u64, Ordering::Relaxed);

        // Process all complete full-size encrypted chunks from the buffer.
        // We deliberately skip the last chunk here — it may be smaller than a full
        // chunk, so we let the post-stream handler deal with it once the stream ends.
        // Each full encrypted chunk = CHUNK_SIZE (plaintext) + 12 (nonce) + 16 (tag).
        while chunk_idx + 1 < chunk_hashes.len() {
            let expected_encrypted_size = CHUNK_SIZE + 12 + 16;

            if buf.len() < expected_encrypted_size {
                break; // Need more data
            }

            let encrypted_chunk: Vec<u8> = buf.drain(..expected_encrypted_size).collect();

            // Verify chunk hash
            let mut chunk_hasher = Sha256::new();
            chunk_hasher.update(&encrypted_chunk);
            let actual_hash = hex::encode(chunk_hasher.finalize());

            if actual_hash != chunk_hashes[chunk_idx] {
                // Try to re-download this specific chunk using Range header
                let redownloaded = retry_chunk(
                    &client, server_url, transfer_id, jwt_token,
                    chunk_idx, &chunk_hashes[chunk_idx],
                ).await?;

                // Decrypt the re-downloaded chunk
                let plaintext = decrypt_chunk(&key, &redownloaded)
                    .map_err(|e| format!("Decrypt failed on retry chunk {}: {}", chunk_idx, e))?;
                output_file.write_all(&plaintext).await
                    .map_err(|e| format!("Write error: {}", e))?;

                full_hasher.update(&redownloaded);
            } else {
                // Hash matches, decrypt and write
                full_hasher.update(&encrypted_chunk);

                let plaintext = decrypt_chunk(&key, &encrypted_chunk)
                    .map_err(|e| format!("Decrypt failed on chunk {}: {}", chunk_idx, e))?;
                output_file.write_all(&plaintext).await
                    .map_err(|e| format!("Write error: {}", e))?;
            }

            chunk_idx += 1;
        }
    }

    // Handle any remaining data in buffer (last chunk)
    if !buf.is_empty() && chunk_idx < chunk_hashes.len() {
        let mut chunk_hasher = Sha256::new();
        chunk_hasher.update(&buf);
        let actual_hash = hex::encode(chunk_hasher.finalize());

        if actual_hash != chunk_hashes[chunk_idx] {
            progress.state.store(STATE_ERROR, Ordering::Relaxed);
            return Err(format!("Final chunk {} hash mismatch", chunk_idx));
        }

        full_hasher.update(&buf);
        let plaintext = decrypt_chunk(&key, &buf)
            .map_err(|e| format!("Decrypt failed on final chunk: {}", e))?;
        output_file.write_all(&plaintext).await
            .map_err(|e| format!("Write error: {}", e))?;
    }

    output_file.flush().await.map_err(|e| format!("Flush error: {}", e))?;

    // Verify full file hash
    let actual_full_hash = hex::encode(full_hasher.finalize());
    if actual_full_hash != file_sha256 {
        progress.state.store(STATE_ERROR, Ordering::Relaxed);
        return Err(format!(
            "Full file hash mismatch: expected {}, got {}",
            file_sha256, actual_full_hash
        ));
    }

    // Confirm download with server
    let _ = client
        .post(format!("{}/transfers/{}/confirm", server_url, transfer_id))
        .header("Authorization", format!("Bearer {}", jwt_token))
        .send()
        .await;

    progress.state.store(STATE_COMPLETE, Ordering::Relaxed);
    Ok(())
}

/// Re-download a specific chunk using HTTP Range.
async fn retry_chunk(
    client: &Client,
    server_url: &str,
    transfer_id: &str,
    jwt_token: &str,
    chunk_idx: usize,
    expected_hash: &str,
) -> Result<Vec<u8>, String> {
    // We need to calculate the byte range for this chunk.
    // Each encrypted chunk is CHUNK_SIZE + 28 bytes (12 nonce + 16 tag),
    // except possibly the last one.
    let encrypted_chunk_size = CHUNK_SIZE + 12 + 16;
    let start = chunk_idx as u64 * encrypted_chunk_size as u64;

    let resp = client
        .get(format!("{}/transfers/{}/data", server_url, transfer_id))
        .header("Authorization", format!("Bearer {}", jwt_token))
        .header("Range", format!("bytes={}-{}", start, start + encrypted_chunk_size as u64 - 1))
        .send()
        .await
        .map_err(|e| format!("Retry chunk {} failed: {}", chunk_idx, e))?;

    let data = resp.bytes().await
        .map_err(|e| format!("Retry chunk {} read failed: {}", chunk_idx, e))?;

    // Verify hash
    let mut hasher = Sha256::new();
    hasher.update(&data);
    let actual_hash = hex::encode(hasher.finalize());

    if actual_hash != expected_hash {
        return Err(format!(
            "Retry chunk {} hash still mismatches: expected {}, got {}",
            chunk_idx, expected_hash, actual_hash
        ));
    }

    Ok(data.to_vec())
}
