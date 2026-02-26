#![allow(private_interfaces)]

pub mod crypto;
pub mod download;
pub mod fast_download;
pub mod fast_upload;
pub mod upload;

use std::ffi::CStr;
use std::os::raw::c_char;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use upload::UploadProgress;
use download::DownloadProgress;

// ── Handle types ────────────────────────────────────────────────────────

enum TransferHandle {
    Upload(Arc<UploadProgress>),
    Download(Arc<DownloadProgress>),
}

/// Opaque handle returned to FFI callers.
type Handle = *mut TransferHandle;

// ── FFI helpers ─────────────────────────────────────────────────────────

unsafe fn cstr_to_str<'a>(ptr: *const c_char) -> &'a str {
    if ptr.is_null() {
        return "";
    }
    unsafe { CStr::from_ptr(ptr).to_str().unwrap_or("") }
}

unsafe fn cstr_to_bytes<'a>(ptr: *const c_char) -> &'a [u8] {
    if ptr.is_null() {
        return &[];
    }
    unsafe { CStr::from_ptr(ptr).to_bytes() }
}

fn get_or_create_runtime() -> &'static tokio::runtime::Runtime {
    use std::sync::OnceLock;
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to create Tokio runtime")
    })
}

// ── FFI exports ─────────────────────────────────────────────────────────

/// Start an upload. Returns a handle for progress polling and cancellation.
///
/// # Safety
/// All string pointers must be valid null-terminated UTF-8 C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn haven_upload_file(
    file_path: *const c_char,
    server_url: *const c_char,
    transfer_id: *const c_char,
    jwt_token: *const c_char,
    master_key: *const c_char,
    salt: *const c_char,
) -> Handle {
    let file_path = unsafe { cstr_to_str(file_path) }.to_string();
    let server_url = unsafe { cstr_to_str(server_url) }.to_string();
    let transfer_id = unsafe { cstr_to_str(transfer_id) }.to_string();
    let jwt_token = unsafe { cstr_to_str(jwt_token) }.to_string();
    let master_key = unsafe { cstr_to_bytes(master_key) }.to_vec();
    let salt = unsafe { cstr_to_bytes(salt) }.to_vec();

    let progress = Arc::new(UploadProgress::new());
    let progress_clone = progress.clone();

    let handle = Box::new(TransferHandle::Upload(progress));
    let handle_ptr = Box::into_raw(handle);

    let rt = get_or_create_runtime();
    rt.spawn(async move {
        let result = upload::upload_file(
            &file_path,
            &server_url,
            &transfer_id,
            &jwt_token,
            &master_key,
            &salt,
            progress_clone.clone(),
        )
        .await;

        if let Err(e) = result {
            eprintln!("Upload error: {}", e);
            *progress_clone.last_error.lock().unwrap() = Some(e);
            // Only overwrite state if it hasn't already been set to a terminal state.
            let cur = progress_clone.state.load(std::sync::atomic::Ordering::Relaxed);
            if cur != upload::STATE_COMPLETE && cur != upload::STATE_CANCELLED {
                progress_clone.state.store(upload::STATE_ERROR, std::sync::atomic::Ordering::Relaxed);
            }
        }
    });

    handle_ptr
}

/// Start a download. Returns a handle for progress polling and cancellation.
///
/// # Safety
/// All string pointers must be valid null-terminated UTF-8 C strings.
/// chunk_hashes_json must be a JSON array of hex strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn haven_download_file(
    save_path: *const c_char,
    server_url: *const c_char,
    transfer_id: *const c_char,
    jwt_token: *const c_char,
    master_key: *const c_char,
    salt: *const c_char,
    file_sha256: *const c_char,
    chunk_hashes_json: *const c_char,
) -> Handle {
    let save_path = unsafe { cstr_to_str(save_path) }.to_string();
    let server_url = unsafe { cstr_to_str(server_url) }.to_string();
    let transfer_id = unsafe { cstr_to_str(transfer_id) }.to_string();
    let jwt_token = unsafe { cstr_to_str(jwt_token) }.to_string();
    let master_key = unsafe { cstr_to_bytes(master_key) }.to_vec();
    let salt = unsafe { cstr_to_bytes(salt) }.to_vec();
    let file_sha256 = unsafe { cstr_to_str(file_sha256) }.to_string();
    let hashes_json = unsafe { cstr_to_str(chunk_hashes_json) }.to_string();

    // CRITICAL: validate chunk_hashes deserialization. unwrap_or_default() silently
    // produces an empty Vec, causing all downloaded data to be discarded and a
    // guaranteed hash mismatch -> STATE_ERROR with no useful message.
    let chunk_hashes: Vec<String> = match serde_json::from_str(&hashes_json) {
        Ok(v) => v,
        Err(e) => {
            let progress = Arc::new(DownloadProgress::new());
            let err_msg = format!(
                "Failed to parse chunk_hashes JSON: {}. Raw input (first 200 chars): '{}'",
                e,
                &hashes_json[..hashes_json.len().min(200)]
            );
            eprintln!("Download error: {}", err_msg);
            *progress.last_error.lock().unwrap() = Some(err_msg);
            progress.state.store(upload::STATE_ERROR, Ordering::Relaxed);
            let handle = Box::new(TransferHandle::Download(progress));
            return Box::into_raw(handle);
        }
    };

    if chunk_hashes.is_empty() {
        let progress = Arc::new(DownloadProgress::new());
        let err_msg = "chunk_hashes is empty — offer data was not received or was corrupted".to_string();
        eprintln!("Download error: {}", err_msg);
        *progress.last_error.lock().unwrap() = Some(err_msg);
        progress.state.store(upload::STATE_ERROR, Ordering::Relaxed);
        let handle = Box::new(TransferHandle::Download(progress));
        return Box::into_raw(handle);
    }

    if file_sha256.is_empty() {
        let progress = Arc::new(DownloadProgress::new());
        let err_msg = "file_sha256 is empty — offer data was not received or was corrupted".to_string();
        eprintln!("Download error: {}", err_msg);
        *progress.last_error.lock().unwrap() = Some(err_msg);
        progress.state.store(upload::STATE_ERROR, Ordering::Relaxed);
        let handle = Box::new(TransferHandle::Download(progress));
        return Box::into_raw(handle);
    }

    let progress = Arc::new(DownloadProgress::new());
    let progress_clone = progress.clone();

    let handle = Box::new(TransferHandle::Download(progress));
    let handle_ptr = Box::into_raw(handle);

    let rt = get_or_create_runtime();
    rt.spawn(async move {
        let result = download::download_file(
            &save_path,
            &server_url,
            &transfer_id,
            &jwt_token,
            &master_key,
            &salt,
            &file_sha256,
            &chunk_hashes,
            progress_clone.clone(),
        )
        .await;

        if let Err(e) = result {
            eprintln!("Download error: {}", e);
            *progress_clone.last_error.lock().unwrap() = Some(e);
            let cur = progress_clone.state.load(std::sync::atomic::Ordering::Relaxed);
            if cur != upload::STATE_COMPLETE && cur != upload::STATE_CANCELLED {
                progress_clone.state.store(upload::STATE_ERROR, std::sync::atomic::Ordering::Relaxed);
            }
        }
    });

    handle_ptr
}

/// Cancel a transfer (upload or download).
///
/// # Safety
/// Handle must be a valid pointer returned by haven_upload_file or haven_download_file.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn haven_transfer_cancel(handle: Handle) {
    if handle.is_null() {
        return;
    }
    let transfer = unsafe { &*handle };
    match transfer {
        TransferHandle::Upload(p) => p.cancelled.store(1, Ordering::Relaxed),
        TransferHandle::Download(p) => p.cancelled.store(1, Ordering::Relaxed),
    }
}

/// Progress result returned by haven_transfer_progress.
#[repr(C)]
pub struct TransferProgressResult {
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub state: u8,
}

/// Poll transfer progress.
///
/// # Safety
/// Handle must be a valid pointer returned by haven_upload_file or haven_download_file.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn haven_transfer_progress(handle: Handle) -> TransferProgressResult {
    if handle.is_null() {
        return TransferProgressResult {
            bytes_done: 0,
            bytes_total: 0,
            state: 0,
        };
    }
    let transfer = unsafe { &*handle };
    match transfer {
        TransferHandle::Upload(p) => TransferProgressResult {
            bytes_done: p.bytes_done.load(Ordering::Relaxed),
            bytes_total: p.bytes_total.load(Ordering::Relaxed),
            state: p.state.load(Ordering::Relaxed),
        },
        TransferHandle::Download(p) => TransferProgressResult {
            bytes_done: p.bytes_done.load(Ordering::Relaxed),
            bytes_total: p.bytes_total.load(Ordering::Relaxed),
            state: p.state.load(Ordering::Relaxed),
        },
    }
}

/// Get the last error message for a transfer.
///
/// Returns a heap-allocated C string with the error message, or NULL if no error.
/// The caller must free the returned string with `haven_free_string`.
///
/// # Safety
/// Handle must be a valid pointer returned by haven_upload_file or haven_download_file.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn haven_get_last_error(handle: Handle) -> *mut c_char {
    if handle.is_null() {
        return std::ptr::null_mut();
    }
    let transfer = unsafe { &*handle };
    let error_opt = match transfer {
        TransferHandle::Upload(p) => p.last_error.lock().unwrap().clone(),
        TransferHandle::Download(p) => p.last_error.lock().unwrap().clone(),
    };
    match error_opt {
        Some(msg) => match std::ffi::CString::new(msg) {
            Ok(s) => s.into_raw(),
            Err(_) => std::ptr::null_mut(),
        },
        None => std::ptr::null_mut(),
    }
}

/// Free a transfer handle.
///
/// # Safety
/// Handle must be a valid pointer returned by haven_upload_file or haven_download_file,
/// and must not be used after this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn haven_transfer_free(handle: Handle) {
    if !handle.is_null() {
        let _ = unsafe { Box::from_raw(handle) };
    }
}

/// Return the upload hashes JSON once pass 1 (hashing) is complete.
///
/// Returns a heap-allocated C string containing
/// `{"file_sha256":"<hex>","chunk_hashes":["<hex>",...]}`,
/// or NULL if hashing is not yet complete.
///
/// The caller must free the returned string with `haven_free_string`.
///
/// # Safety
/// Handle must be a valid pointer returned by `haven_upload_file`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn haven_upload_hashes_json(handle: Handle) -> *mut std::os::raw::c_char {
    if handle.is_null() {
        return std::ptr::null_mut();
    }
    match unsafe { &*handle } {
        TransferHandle::Upload(p) => {
            let guard = p.hashes_json.lock().unwrap();
            match guard.as_ref() {
                Some(json) => match std::ffi::CString::new(json.as_str()) {
                    Ok(s) => s.into_raw(),
                    Err(_) => std::ptr::null_mut(),
                },
                None => std::ptr::null_mut(),
            }
        }
        _ => std::ptr::null_mut(),
    }
}

// ── Fast transfer FFI exports ──────────────────────────────────────────

/// Start a fast UDP blast upload. Returns a handle for progress polling.
///
/// # Safety
/// All string pointers must be valid null-terminated UTF-8 C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn haven_fast_upload(
    file_path: *const c_char,
    server_url: *const c_char,
    transfer_id: *const c_char,
    jwt_token: *const c_char,
    master_key: *const c_char,
    salt: *const c_char,
) -> Handle {
    let file_path = unsafe { cstr_to_str(file_path) }.to_string();
    let server_url = unsafe { cstr_to_str(server_url) }.to_string();
    let transfer_id = unsafe { cstr_to_str(transfer_id) }.to_string();
    let jwt_token = unsafe { cstr_to_str(jwt_token) }.to_string();
    let master_key = unsafe { cstr_to_bytes(master_key) }.to_vec();
    let salt = unsafe { cstr_to_bytes(salt) }.to_vec();

    let progress = Arc::new(UploadProgress::new());
    let progress_clone = progress.clone();

    let handle = Box::new(TransferHandle::Upload(progress));
    let handle_ptr = Box::into_raw(handle);

    let rt = get_or_create_runtime();
    rt.spawn(async move {
        let result = fast_upload::fast_upload_file(
            &file_path,
            &server_url,
            &transfer_id,
            &jwt_token,
            &master_key,
            &salt,
            progress_clone.clone(),
        )
        .await;

        if let Err(e) = result {
            eprintln!("Fast upload error: {}", e);
            *progress_clone.last_error.lock().unwrap() = Some(e);
            let cur = progress_clone.state.load(std::sync::atomic::Ordering::Relaxed);
            if cur != upload::STATE_COMPLETE && cur != upload::STATE_CANCELLED {
                progress_clone.state.store(upload::STATE_ERROR, std::sync::atomic::Ordering::Relaxed);
            }
        }
    });

    handle_ptr
}

/// Start a fast UDP blast download. Returns a handle for progress polling.
///
/// # Safety
/// All string pointers must be valid null-terminated UTF-8 C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn haven_fast_download(
    save_path: *const c_char,
    server_url: *const c_char,
    transfer_id: *const c_char,
    jwt_token: *const c_char,
    master_key: *const c_char,
    salt: *const c_char,
    file_sha256: *const c_char,
    chunk_hashes_json: *const c_char,
) -> Handle {
    let save_path = unsafe { cstr_to_str(save_path) }.to_string();
    let server_url = unsafe { cstr_to_str(server_url) }.to_string();
    let transfer_id = unsafe { cstr_to_str(transfer_id) }.to_string();
    let jwt_token = unsafe { cstr_to_str(jwt_token) }.to_string();
    let master_key = unsafe { cstr_to_bytes(master_key) }.to_vec();
    let salt = unsafe { cstr_to_bytes(salt) }.to_vec();
    let file_sha256 = unsafe { cstr_to_str(file_sha256) }.to_string();
    let hashes_json = unsafe { cstr_to_str(chunk_hashes_json) }.to_string();

    let chunk_hashes: Vec<String> = match serde_json::from_str(&hashes_json) {
        Ok(v) => v,
        Err(e) => {
            let progress = Arc::new(DownloadProgress::new());
            let err_msg = format!("Failed to parse chunk_hashes: {}", e);
            eprintln!("Fast download error: {}", err_msg);
            *progress.last_error.lock().unwrap() = Some(err_msg);
            progress.state.store(upload::STATE_ERROR, Ordering::Relaxed);
            let handle = Box::new(TransferHandle::Download(progress));
            return Box::into_raw(handle);
        }
    };

    if chunk_hashes.is_empty() || file_sha256.is_empty() {
        let progress = Arc::new(DownloadProgress::new());
        *progress.last_error.lock().unwrap() = Some("Empty hashes or sha256".into());
        progress.state.store(upload::STATE_ERROR, Ordering::Relaxed);
        let handle = Box::new(TransferHandle::Download(progress));
        return Box::into_raw(handle);
    }

    let progress = Arc::new(DownloadProgress::new());
    let progress_clone = progress.clone();

    let handle = Box::new(TransferHandle::Download(progress));
    let handle_ptr = Box::into_raw(handle);

    let rt = get_or_create_runtime();
    rt.spawn(async move {
        let result = fast_download::fast_download_file(
            &save_path,
            &server_url,
            &transfer_id,
            &jwt_token,
            &master_key,
            &salt,
            &file_sha256,
            &chunk_hashes,
            progress_clone.clone(),
        )
        .await;

        if let Err(e) = result {
            eprintln!("Fast download error: {}", e);
            *progress_clone.last_error.lock().unwrap() = Some(e);
            let cur = progress_clone.state.load(std::sync::atomic::Ordering::Relaxed);
            if cur != upload::STATE_COMPLETE && cur != upload::STATE_CANCELLED {
                progress_clone.state.store(upload::STATE_ERROR, std::sync::atomic::Ordering::Relaxed);
            }
        }
    });

    handle_ptr
}

/// Free a C string returned by `haven_upload_hashes_json` or `haven_get_last_error`.
///
/// # Safety
/// `ptr` must be a non-null pointer previously returned by one of the string-returning FFI functions.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn haven_free_string(ptr: *mut std::os::raw::c_char) {
    if !ptr.is_null() {
        drop(unsafe { std::ffi::CString::from_raw(ptr) });
    }
}
