use std::sync::Arc;

use axum::{
    Extension,
    body::Bytes,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Serialize;
use tokio::io::AsyncWriteExt;
use tracing::error;
use uuid::Uuid;

use crate::auth::AppStateInner;
use crate::middleware::Claims;

/// #5: 10 MB upload limit for files (was 50 MB).
/// The server-level DefaultBodyLimit is also set to 10 MB by default,
/// configurable via HAVEN_MAX_BODY_SIZE env var.
const MAX_FILE_SIZE: usize = 10 * 1024 * 1024;

#[derive(Serialize)]
pub struct UploadResponse {
    pub file_id: String,
    pub size: u64,
}

/// POST /files — accepts raw encrypted bytes (application/octet-stream),
/// saves to {uploads_dir}/{id}, inserts DB row, returns { file_id, size }.
///
/// #8: Channel authorization model — all authenticated users can access all files.
/// This is by design for the current MVP: Haven is a small private server where
/// all registered users are trusted. Per-channel file ACLs are a future feature.
pub async fn upload_file(
    State(state): State<Arc<AppStateInner>>,
    Extension(claims): Extension<Claims>,
    bytes: Bytes,
) -> Result<impl IntoResponse, StatusCode> {
    if bytes.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    if bytes.len() > MAX_FILE_SIZE {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }

    let file_id = Uuid::new_v4().to_string();
    let size = bytes.len() as i64;

    // #14: Use configurable uploads directory from state
    let uploads_dir = &state.uploads_dir;
    tokio::fs::create_dir_all(uploads_dir)
        .await
        .map_err(|e| {
            error!("Failed to create uploads directory: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Write encrypted blob to disk
    let file_path = uploads_dir.join(&file_id);
    let mut file = tokio::fs::File::create(&file_path).await.map_err(|e| {
        error!("Failed to create file {:?}: {}", file_path, e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    file.write_all(&bytes).await.map_err(|e| {
        error!("Failed to write file {:?}: {}", file_path, e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Insert DB record
    let db = state.clone();
    let fid = file_id.clone();
    let uid = claims.sub.to_string();
    tokio::task::spawn_blocking(move || db.db.insert_file(&fid, &uid, "upload", size))
        .await
        .map_err(|e| {
            error!("spawn_blocking join error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .map_err(|e| {
            error!("DB insert_file error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok((
        StatusCode::CREATED,
        axum::Json(UploadResponse {
            file_id,
            size: size as u64,
        }),
    ))
}

/// GET /files/{file_id} — reads file from disk, streams back the encrypted blob.
///
/// #8: All authenticated users can download any file — see upload_file doc comment.
pub async fn download_file(
    State(state): State<Arc<AppStateInner>>,
    Path(file_id): Path<String>,
    Extension(_claims): Extension<Claims>,
) -> Result<impl IntoResponse, StatusCode> {
    // Validate file_id is a valid UUID to prevent path traversal
    file_id
        .parse::<Uuid>()
        .map_err(|_| StatusCode::BAD_REQUEST)?;

    // Verify file exists in DB
    let db = state.clone();
    let fid = file_id.clone();
    let file_row = tokio::task::spawn_blocking(move || db.db.get_file(&fid))
        .await
        .map_err(|e| {
            error!("spawn_blocking join error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .map_err(|e| {
            error!("DB get_file error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if file_row.is_none() {
        return Err(StatusCode::NOT_FOUND);
    }

    // #14: Use configurable uploads directory from state
    let file_path = state.uploads_dir.join(&file_id);
    let bytes = tokio::fs::read(&file_path).await.map_err(|e| {
        error!("Failed to read file {:?}: {}", file_path, e);
        StatusCode::NOT_FOUND
    })?;

    Ok((
        [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
        bytes,
    ))
}
