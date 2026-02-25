use std::sync::Arc;

use axum::{
    Json,
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
use tracing::{info, warn};

use crate::db::FileDb;
use crate::storage::Storage;

/// Shared application state for all route handlers.
#[derive(Clone)]
pub struct AppState {
    pub db: Arc<FileDb>,
    pub storage: Arc<Storage>,
    pub jwt_secret: String,
    pub retention_hours: u64,
}

/// JWT claims — must match the messaging server's format.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct Claims {
    pub sub: String,     // user ID
    pub username: String,
    pub exp: usize,
}

// ── Request/response types ──────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CreateTransferRequest {
    pub id: String,
    pub file_size: u64,
    pub chunk_size: Option<u64>,
    pub file_sha256: String,
    pub chunk_hashes: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct CreateTransferResponse {
    pub id: String,
    pub chunk_count: usize,
}

#[derive(Debug, Serialize)]
pub struct TransferStatus {
    pub id: String,
    pub status: String,
    pub file_size: u64,
    pub bytes_received: u64,
    pub chunk_count: u64,
    pub created_at: String,
}

// ── Auth helper ─────────────────────────────────────────────────────────

fn extract_claims(headers: &HeaderMap, jwt_secret: &str) -> Result<Claims, StatusCode> {
    let auth_header = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let token_data = jsonwebtoken::decode::<Claims>(
        auth_header,
        &jsonwebtoken::DecodingKey::from_secret(jwt_secret.as_bytes()),
        &jsonwebtoken::Validation::default(),
    )
    .map_err(|_| StatusCode::UNAUTHORIZED)?;

    Ok(token_data.claims)
}

// ── Handlers ────────────────────────────────────────────────────────────

/// POST /transfers — create a new transfer record with file metadata + chunk hashes.
pub async fn create_transfer(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateTransferRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let claims = extract_claims(&headers, &state.jwt_secret)?;
    let chunk_size = req.chunk_size.unwrap_or(4_194_304); // 4 MB default
    let chunk_count = req.chunk_hashes.len();

    // Validate chunk count matches file size
    let expected_chunks = if req.file_size == 0 {
        1
    } else {
        ((req.file_size + chunk_size - 1) / chunk_size) as usize
    };
    if chunk_count != expected_chunks {
        warn!(
            "Chunk count mismatch: got {} hashes, expected {} for {} bytes with {} chunk size",
            chunk_count, expected_chunks, req.file_size, chunk_size
        );
        return Err(StatusCode::BAD_REQUEST);
    }

    let retention_hours = state.retention_hours;
    let transfer_id = req.id.clone();

    // Create DB record
    state.db.with_conn_mut(|conn| {
        conn.execute(
            "INSERT INTO transfers (id, uploader_id, file_size, chunk_size, chunk_count, file_sha256, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now', '+' || ?7 || ' hours'))",
            rusqlite::params![
                &req.id,
                &claims.sub,
                req.file_size as i64,
                chunk_size as i64,
                chunk_count as i64,
                &req.file_sha256,
                retention_hours as i64,
            ],
        )?;

        // Insert chunk records
        let mut offset: u64 = 0;
        for (i, hash) in req.chunk_hashes.iter().enumerate() {
            let length = if i == chunk_count - 1 {
                // Last chunk may be smaller
                req.file_size - offset
            } else {
                chunk_size
            };
            conn.execute(
                "INSERT INTO chunks (transfer_id, chunk_index, sha256, byte_offset, byte_length)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![&req.id, i as i64, hash, offset as i64, length as i64],
            )?;
            offset += length;
        }
        Ok(())
    }).map_err(|e| {
        warn!("Failed to create transfer: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Pre-allocate file on disk
    state.storage.create_file(&transfer_id, req.file_size).await.map_err(|e| {
        warn!("Failed to create file: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    info!(
        "Transfer {} created by {}: {} bytes, {} chunks",
        transfer_id, claims.username, req.file_size, chunk_count
    );

    Ok((
        StatusCode::CREATED,
        Json(CreateTransferResponse {
            id: transfer_id,
            chunk_count,
        }),
    ))
}

/// PUT /transfers/{id}/data — streaming upload.
///
/// The body is the raw encrypted file data, written sequentially chunk by chunk.
/// The server verifies each chunk's SHA-256 hash as it arrives.
pub async fn upload_data(
    State(state): State<AppState>,
    Path(transfer_id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Result<StatusCode, StatusCode> {
    let claims = extract_claims(&headers, &state.jwt_secret)?;

    // Verify transfer exists and caller is the uploader
    let (chunk_size, file_size, uploader_id, current_status): (u64, u64, String, String) =
        state.db.with_conn(|conn| {
            conn.query_row(
                "SELECT chunk_size, file_size, uploader_id, status FROM transfers WHERE id = ?1",
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
        .map_err(|_| StatusCode::NOT_FOUND)?;

    if uploader_id != claims.sub {
        return Err(StatusCode::FORBIDDEN);
    }
    if current_status != "uploading" {
        return Err(StatusCode::CONFLICT);
    }

    // Load chunk metadata (expected hashes, offsets, lengths)
    let chunks: Vec<(i64, String, u64, u64, bool)> = state.db.with_conn(|conn| {
        let mut stmt = conn.prepare(
            "SELECT chunk_index, sha256, byte_offset, byte_length, received
             FROM chunks WHERE transfer_id = ?1 ORDER BY chunk_index"
        )?;
        let rows = stmt.query_map([&transfer_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)? as u64,
                row.get::<_, i64>(3)? as u64,
                row.get::<_, bool>(4)?,
            ))
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Stream the body, splitting into chunk-sized pieces and verifying hashes
    let mut stream = http_body_util::BodyStream::new(body);
    use futures_util::StreamExt;

    let mut buf = Vec::with_capacity(chunk_size as usize);
    let mut chunk_idx: usize = 0;
    let mut total_received: u64 = 0;

    // Process all complete chunks out of `buf`. Requires a full `len` bytes
    // for every chunk including the last — avoids hashing partial data.
    macro_rules! flush_chunks {
        () => {
            while chunk_idx < chunks.len() {
                let (_, ref expected_hash, offset, length, already_received) = chunks[chunk_idx];
                let len = length as usize;

                if buf.len() < len {
                    break;
                }

                let chunk_data: Vec<u8> = buf.drain(..len).collect();

                if !already_received {
                    state
                        .storage
                        .write_chunk(&transfer_id, offset, expected_hash, &chunk_data)
                        .await
                        .map_err(|e| {
                            warn!("Chunk {} hash verification failed: {}", chunk_idx, e);
                            StatusCode::BAD_REQUEST
                        })?;

                    total_received += chunk_data.len() as u64;
                    let tid = transfer_id.clone();
                    let ci = chunk_idx as i64;
                    let tr = total_received as i64;
                    state.db.with_conn_mut(move |conn| {
                        conn.execute(
                            "UPDATE chunks SET received = 1 WHERE transfer_id = ?1 AND chunk_index = ?2",
                            rusqlite::params![&tid, ci],
                        )?;
                        conn.execute(
                            "UPDATE transfers SET bytes_received = ?1 WHERE id = ?2",
                            rusqlite::params![tr, &tid],
                        )?;
                        Ok(())
                    }).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
                }

                if chunk_idx % 100 == 0 {
                    info!(
                        "Transfer {}: chunk {}/{} received ({} bytes total)",
                        &transfer_id, chunk_idx, chunks.len(), total_received
                    );
                }

                chunk_idx += 1;
            }
        };
    }

    while let Some(frame_result) = stream.next().await {
        let frame = frame_result.map_err(|_| StatusCode::BAD_REQUEST)?;
        if let Ok(data) = frame.into_data() {
            buf.extend_from_slice(&data);
            flush_chunks!();
        }
    }

    // Stream exhausted — process the last chunk from whatever remains in buf.
    flush_chunks!();

    // If we received all chunks, mark as complete
    if chunk_idx >= chunks.len() {
        let tid = transfer_id.clone();
        state.db.with_conn_mut(move |conn| {
            conn.execute(
                "UPDATE transfers SET status = 'complete', bytes_received = file_size WHERE id = ?1",
                [&tid],
            )?;
            Ok(())
        }).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        info!("Transfer {} complete ({} bytes)", transfer_id, file_size);
    }

    Ok(StatusCode::OK)
}

/// PUT /transfers/{id}/chunks/{index} — upload a single chunk (parallel-safe).
///
/// Idempotent: if the chunk is already received, returns 200 immediately.
/// Optimized hot path: single JOIN query, direct file I/O (no SHA-256 re-check),
/// no bytes_received update (set only on completion).
pub async fn upload_chunk(
    State(state): State<AppState>,
    Path((transfer_id, chunk_index)): Path<(String, i64)>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, StatusCode> {
    let claims = extract_claims(&headers, &state.jwt_secret)?;

    // Single JOIN query: transfer auth/status + chunk metadata in one round-trip
    let (uploader_id, current_status, offset, byte_length, already_received): (String, String, u64, u64, bool) = state
        .db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT t.uploader_id, t.status, c.byte_offset, c.byte_length, c.received
                 FROM transfers t
                 JOIN chunks c ON c.transfer_id = t.id
                 WHERE t.id = ?1 AND c.chunk_index = ?2",
                rusqlite::params![&transfer_id, chunk_index],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)? as u64,
                        row.get::<_, i64>(3)? as u64,
                        row.get::<_, bool>(4)?,
                    ))
                },
            )
            .map_err(|_| anyhow::anyhow!("Transfer or chunk not found"))
        })
        .map_err(|_| StatusCode::NOT_FOUND)?;

    if uploader_id != claims.sub {
        return Err(StatusCode::FORBIDDEN);
    }
    if current_status != "uploading" {
        return Err(StatusCode::CONFLICT);
    }

    // Idempotent: already received -> 200
    if already_received {
        return Ok(StatusCode::OK);
    }

    // Validate body length matches expected chunk size
    if body.len() as u64 != byte_length {
        warn!(
            "Chunk {} body length {} != expected {}",
            chunk_index,
            body.len(),
            byte_length
        );
        return Err(StatusCode::BAD_REQUEST);
    }

    // Write directly to pre-allocated file — no SHA-256 re-verification, no flush.
    // Client already verified chunk hashes during encryption pass.
    {
        let path = state.storage.file_path(&transfer_id);
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .await
            .map_err(|e| {
                warn!("Failed to open file for chunk {}: {}", chunk_index, e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
        use tokio::io::{AsyncSeekExt, AsyncWriteExt};
        file.seek(std::io::SeekFrom::Start(offset))
            .await
            .map_err(|e| {
                warn!("Failed to seek for chunk {}: {}", chunk_index, e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
        file.write_all(&body).await.map_err(|e| {
            warn!("Failed to write chunk {}: {}", chunk_index, e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
        // No flush — OS page cache coalesces writes for pre-allocated file
    }

    // Mark chunk received + check completion (2 ops, no bytes_received update)
    let tid = transfer_id.clone();
    state
        .db
        .with_conn_mut(move |conn| {
            conn.execute(
                "UPDATE chunks SET received = 1 WHERE transfer_id = ?1 AND chunk_index = ?2",
                rusqlite::params![&tid, chunk_index],
            )?;

            let unreceived: i64 = conn.query_row(
                "SELECT COUNT(*) FROM chunks WHERE transfer_id = ?1 AND received = 0",
                [&tid],
                |r| r.get(0),
            )?;
            if unreceived == 0 {
                conn.execute(
                    "UPDATE transfers SET status = 'complete', bytes_received = file_size WHERE id = ?1",
                    [&tid],
                )?;
            }

            Ok(())
        })
        .map_err(|e| {
            warn!("DB update failed for chunk {}: {}", chunk_index, e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(StatusCode::OK)
}

/// GET /transfers/{id}/data — streaming download.
///
/// Supports HTTP Range header for resume. Serves bytes up to `bytes_received`,
/// allowing the receiver to start downloading before the upload completes.
pub async fn download_data(
    State(state): State<AppState>,
    Path(transfer_id): Path<String>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, StatusCode> {
    let _claims = extract_claims(&headers, &state.jwt_secret)?;

    // Get transfer info
    let (file_size, bytes_received, status): (u64, u64, String) = state.db.with_conn(|conn| {
        conn.query_row(
            "SELECT file_size, bytes_received, status FROM transfers WHERE id = ?1",
            [&transfer_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)? as u64,
                    row.get::<_, i64>(1)? as u64,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .map_err(|_| anyhow::anyhow!("Transfer not found"))
    }).map_err(|_| StatusCode::NOT_FOUND)?;

    if status == "expired" {
        return Err(StatusCode::GONE);
    }

    // Parse Range header for resume support
    let start_offset = parse_range_start(&headers).unwrap_or(0);

    // Determine how many bytes are available to serve
    let available = if status == "complete" { file_size } else { bytes_received };
    if start_offset >= available {
        return Err(StatusCode::RANGE_NOT_SATISFIABLE);
    }

    let content_length = available - start_offset;
    let transfer_id_owned = transfer_id.clone();
    let storage = state.storage.clone();

    // Stream the file from disk
    let stream = async_stream::stream! {
        let path = storage.file_path(&transfer_id_owned);
        let mut file = match tokio::fs::File::open(&path).await {
            Ok(f) => f,
            Err(e) => {
                yield Err(std::io::Error::new(std::io::ErrorKind::Other, e));
                return;
            }
        };

        if start_offset > 0 {
            use tokio::io::AsyncSeekExt;
            if let Err(e) = file.seek(std::io::SeekFrom::Start(start_offset)).await {
                yield Err(e);
                return;
            }
        }

        let mut remaining = content_length;
        let mut buf = vec![0u8; 64 * 1024]; // 64 KB read buffer
        while remaining > 0 {
            let to_read = (remaining as usize).min(buf.len());
            match file.read(&mut buf[..to_read]).await {
                Ok(0) => break,
                Ok(n) => {
                    remaining -= n as u64;
                    yield Ok(Bytes::copy_from_slice(&buf[..n]));
                }
                Err(e) => {
                    yield Err(e);
                    return;
                }
            }
        }
    };

    let body = Body::from_stream(stream);

    let mut response_headers = HeaderMap::new();
    response_headers.insert(header::CONTENT_TYPE, "application/octet-stream".parse().unwrap());
    response_headers.insert(header::CONTENT_LENGTH, content_length.to_string().parse().unwrap());
    response_headers.insert(header::ACCEPT_RANGES, "bytes".parse().unwrap());

    if start_offset > 0 {
        response_headers.insert(
            header::CONTENT_RANGE,
            format!("bytes {}-{}/{}", start_offset, available - 1, file_size)
                .parse()
                .unwrap(),
        );
        Ok((StatusCode::PARTIAL_CONTENT, response_headers, body))
    } else {
        Ok((StatusCode::OK, response_headers, body))
    }
}

/// GET /transfers/{id} — transfer status.
pub async fn get_transfer_status(
    State(state): State<AppState>,
    Path(transfer_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<TransferStatus>, StatusCode> {
    let _claims = extract_claims(&headers, &state.jwt_secret)?;

    let status = state.db.with_conn(|conn| {
        conn.query_row(
            "SELECT id, status, file_size, bytes_received, chunk_count, created_at
             FROM transfers WHERE id = ?1",
            [&transfer_id],
            |row| {
                Ok(TransferStatus {
                    id: row.get(0)?,
                    status: row.get(1)?,
                    file_size: row.get::<_, i64>(2)? as u64,
                    bytes_received: row.get::<_, i64>(3)? as u64,
                    chunk_count: row.get::<_, i64>(4)? as u64,
                    created_at: row.get(5)?,
                })
            },
        )
        .map_err(|_| anyhow::anyhow!("Transfer not found"))
    }).map_err(|_| StatusCode::NOT_FOUND)?;

    Ok(Json(status))
}

/// POST /transfers/{id}/confirm — receiver confirms successful download.
/// Server deletes the file from disk.
pub async fn confirm_transfer(
    State(state): State<AppState>,
    Path(transfer_id): Path<String>,
    headers: HeaderMap,
) -> Result<StatusCode, StatusCode> {
    let _claims = extract_claims(&headers, &state.jwt_secret)?;

    // Delete file from disk
    state.storage.delete_file(&transfer_id).await.map_err(|e| {
        warn!("Failed to delete file for {}: {}", transfer_id, e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Mark as confirmed in DB
    state.db.with_conn_mut(|conn| {
        conn.execute(
            "UPDATE transfers SET status = 'confirmed' WHERE id = ?1",
            [&transfer_id],
        )?;
        Ok(())
    }).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    info!("Transfer {} confirmed and file deleted", transfer_id);
    Ok(StatusCode::OK)
}

/// DELETE /transfers/{id} — delete a transfer (uploader only).
pub async fn delete_transfer(
    State(state): State<AppState>,
    Path(transfer_id): Path<String>,
    headers: HeaderMap,
) -> Result<StatusCode, StatusCode> {
    let claims = extract_claims(&headers, &state.jwt_secret)?;

    // Verify ownership
    let uploader_id: String = state.db.with_conn(|conn| {
        conn.query_row(
            "SELECT uploader_id FROM transfers WHERE id = ?1",
            [&transfer_id],
            |row| row.get(0),
        )
        .map_err(|_| anyhow::anyhow!("Transfer not found"))
    }).map_err(|_| StatusCode::NOT_FOUND)?;

    if uploader_id != claims.sub {
        return Err(StatusCode::FORBIDDEN);
    }

    // Delete file from disk
    state.storage.delete_file(&transfer_id).await.ok();

    // Delete from DB (CASCADE deletes chunks too)
    state.db.with_conn_mut(|conn| {
        conn.execute("DELETE FROM transfers WHERE id = ?1", [&transfer_id])?;
        Ok(())
    }).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    info!("Transfer {} deleted by {}", transfer_id, claims.username);
    Ok(StatusCode::OK)
}

/// GET /health — liveness check (no auth).
pub async fn health() -> &'static str {
    "ok"
}

// ── Helpers ─────────────────────────────────────────────────────────────

fn parse_range_start(headers: &HeaderMap) -> Option<u64> {
    let range = headers.get(header::RANGE)?.to_str().ok()?;
    // Parse "bytes=START-" or "bytes=START-END"
    let range = range.strip_prefix("bytes=")?;
    let start_str = range.split('-').next()?;
    start_str.parse().ok()
}
