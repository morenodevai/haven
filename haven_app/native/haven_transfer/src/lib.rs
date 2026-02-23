/// Haven Transfer Protocol — high-speed encrypted UDP file transfer.
///
/// FFI exports for Flutter/Dart integration via dart:ffi.
/// Control messages go over the existing WebSocket on port 3210.
/// Data packets go over UDP on port 3211.

pub mod congestion;
pub mod crypto;
pub mod protocol;
pub mod receiver;
pub mod sender;
pub mod stats;

use std::collections::HashMap;
use std::ffi::CStr;
use std::net::SocketAddr;
use std::os::raw::c_char;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::{bounded, Sender as ChannelSender};
use parking_lot::Mutex;

use crate::congestion::CongestionConfig;
use crate::receiver::ReceiverEvent;
use crate::sender::SenderCommand;
use crate::stats::TransferStats;

// ── Session tracking ──

static NEXT_SESSION_ID: AtomicU32 = AtomicU32::new(1);

struct ActiveTransfer {
    stats: Arc<TransferStats>,
    cancelled: Arc<AtomicBool>,
    sender_cmd_tx: Option<ChannelSender<SenderCommand>>,
    #[allow(dead_code)]
    thread_handle: Option<std::thread::JoinHandle<()>>,
}

use std::sync::OnceLock;

static SESSIONS: OnceLock<Mutex<HashMap<u32, ActiveTransfer>>> = OnceLock::new();

fn sessions() -> &'static Mutex<HashMap<u32, ActiveTransfer>> {
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

// ── FFI Exports ──

/// Start a file send. Returns session_id (> 0) on success, 0 on failure.
///
/// # Safety
/// All pointer params must be valid. `jwt_token` can be null to skip auth.
#[no_mangle]
pub unsafe extern "C" fn htp_send_file(
    file_path: *const c_char,
    dest_addr: *const c_char,
    master_key: *const u8,
    salt: *const u8,
    jwt_token: *const c_char,
) -> u32 {
    let path = match CStr::from_ptr(file_path).to_str() {
        Ok(s) => s.to_string(),
        Err(_) => return 0,
    };
    let addr_str = match CStr::from_ptr(dest_addr).to_str() {
        Ok(s) => s,
        Err(_) => return 0,
    };
    let addr: SocketAddr = match addr_str.parse() {
        Ok(a) => a,
        Err(_) => return 0,
    };
    let key: [u8; 32] = std::slice::from_raw_parts(master_key, 32)
        .try_into()
        .unwrap();
    let salt_arr: [u8; 32] = std::slice::from_raw_parts(salt, 32)
        .try_into()
        .unwrap();
    let token = if jwt_token.is_null() {
        String::new()
    } else {
        CStr::from_ptr(jwt_token).to_str().unwrap_or("").to_string()
    };

    let session_id = NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed);
    let stats = Arc::new(TransferStats::new());
    let cancelled = Arc::new(AtomicBool::new(false));

    let (cmd_tx, cmd_rx) = bounded::<SenderCommand>(256);

    let config = sender::SendConfig {
        session_id,
        file_path: path,
        dest_addr: addr,
        master_key: key,
        salt: salt_arr,
        congestion: CongestionConfig::default(),
        jwt_token: token,
    };

    let stats_clone = stats.clone();
    let cancelled_clone = cancelled.clone();

    let handle = std::thread::Builder::new()
        .name(format!("htp-send-{}", session_id))
        .spawn(move || {
            match sender::run_sender(config, cmd_rx, stats_clone, cancelled_clone) {
                Ok(result) => {
                    log::info!(
                        "Transfer {} complete: {} bytes in {:.1}s ({} B/s)",
                        session_id,
                        result.total_bytes,
                        result.elapsed.as_secs_f64(),
                        result.throughput_bps
                    );
                }
                Err(e) => {
                    log::error!("Transfer {} failed: {}", session_id, e);
                }
            }
            sessions().lock().remove(&session_id);
        });

    match handle {
        Ok(h) => {
            sessions().lock().insert(
                session_id,
                ActiveTransfer {
                    stats,
                    cancelled,
                    sender_cmd_tx: Some(cmd_tx),
                    thread_handle: Some(h),
                },
            );
            session_id
        }
        Err(_) => 0,
    }
}

/// Start receiving a file. Returns session_id (> 0) on success, 0 on failure.
///
/// # Safety
/// All pointer params must be valid. `jwt_token` can be null to skip auth.
/// `relay_addr` can be null if no auth needed.
#[no_mangle]
pub unsafe extern "C" fn htp_recv_file(
    session_id: u32,
    output_path: *const c_char,
    file_size: u64,
    total_chunks: u64,
    listen_addr: *const c_char,
    master_key: *const u8,
    salt: *const u8,
    jwt_token: *const c_char,
    relay_addr: *const c_char,
    nack_callback: Option<extern "C" fn(session_id: u32, missing: *const u64, count: u32)>,
    done_callback: Option<extern "C" fn(session_id: u32, total_bytes: u64)>,
) -> u32 {
    let path = match CStr::from_ptr(output_path).to_str() {
        Ok(s) => s.to_string(),
        Err(_) => return 0,
    };
    let addr_str = match CStr::from_ptr(listen_addr).to_str() {
        Ok(s) => s,
        Err(_) => return 0,
    };
    let addr: SocketAddr = match addr_str.parse() {
        Ok(a) => a,
        Err(_) => return 0,
    };
    let key: [u8; 32] = std::slice::from_raw_parts(master_key, 32)
        .try_into()
        .unwrap();
    let salt_arr: [u8; 32] = std::slice::from_raw_parts(salt, 32)
        .try_into()
        .unwrap();
    let token = if jwt_token.is_null() {
        String::new()
    } else {
        CStr::from_ptr(jwt_token).to_str().unwrap_or("").to_string()
    };
    let relay: Option<SocketAddr> = if relay_addr.is_null() {
        None
    } else {
        CStr::from_ptr(relay_addr)
            .to_str()
            .ok()
            .and_then(|s| s.parse().ok())
    };

    let stats = Arc::new(TransferStats::new());
    let cancelled = Arc::new(AtomicBool::new(false));

    let (event_tx, event_rx) = bounded::<ReceiverEvent>(256);

    let config = receiver::RecvConfig {
        session_id,
        output_path: path,
        file_size,
        total_chunks,
        listen_addr: addr,
        master_key: key,
        salt: salt_arr,
        jwt_token: token,
        relay_addr: relay,
    };

    let stats_clone = stats.clone();
    let cancelled_clone = cancelled.clone();

    let handle = std::thread::Builder::new()
        .name(format!("htp-recv-{}", session_id))
        .spawn(move || {
            match receiver::run_receiver(config, event_tx, stats_clone, cancelled_clone) {
                Ok(result) => {
                    log::info!(
                        "Receive {} complete: {} bytes in {:.1}s ({} B/s)",
                        session_id,
                        result.total_bytes,
                        result.elapsed.as_secs_f64(),
                        result.throughput_bps
                    );
                }
                Err(e) => {
                    log::error!("Receive {} failed: {}", session_id, e);
                }
            }
            sessions().lock().remove(&session_id);
        });

    if handle.is_err() {
        return 0;
    }

    // Event dispatcher thread
    let nack_cb = nack_callback;
    let done_cb = done_callback;
    std::thread::Builder::new()
        .name(format!("htp-recv-events-{}", session_id))
        .spawn(move || {
            while let Ok(event) = event_rx.recv() {
                match event {
                    ReceiverEvent::Nack(missing) => {
                        if let Some(cb) = nack_cb {
                            cb(session_id, missing.as_ptr(), missing.len() as u32);
                        }
                    }
                    ReceiverEvent::Done { total_bytes } => {
                        if let Some(cb) = done_cb {
                            cb(session_id, total_bytes);
                        }
                        break;
                    }
                    ReceiverEvent::Progress { .. } => {}
                }
            }
        })
        .ok();

    sessions().lock().insert(
        session_id,
        ActiveTransfer {
            stats,
            cancelled,
            sender_cmd_tx: None,
            thread_handle: handle.ok(),
        },
    );

    session_id
}

/// Feed a NACK to the sender.
#[no_mangle]
pub extern "C" fn htp_sender_nack(session_id: u32, missing: *const u64, count: u32) {
    let seqs: Vec<u64> = unsafe { std::slice::from_raw_parts(missing, count as usize).to_vec() };
    if let Some(transfer) = sessions().lock().get(&session_id) {
        if let Some(tx) = &transfer.sender_cmd_tx {
            let _ = tx.send(SenderCommand::Nack(seqs));
        }
    }
}

/// Feed an RTT sample to the sender's congestion controller.
#[no_mangle]
pub extern "C" fn htp_sender_rtt(session_id: u32, rtt_microseconds: u64) {
    if let Some(transfer) = sessions().lock().get(&session_id) {
        if let Some(tx) = &transfer.sender_cmd_tx {
            let _ = tx.send(SenderCommand::RttSample(Duration::from_micros(rtt_microseconds)));
        }
    }
}

/// Tell the sender the receiver has acknowledged up to this sequence.
#[no_mangle]
pub extern "C" fn htp_sender_ack(session_id: u32, up_to_sequence: u64) {
    if let Some(transfer) = sessions().lock().get(&session_id) {
        if let Some(tx) = &transfer.sender_cmd_tx {
            let _ = tx.send(SenderCommand::AckUpTo(up_to_sequence));
        }
    }
}

/// Cancel a transfer.
#[no_mangle]
pub extern "C" fn htp_cancel(session_id: u32) {
    if let Some(transfer) = sessions().lock().get(&session_id) {
        transfer.cancelled.store(true, Ordering::Relaxed);
        if let Some(tx) = &transfer.sender_cmd_tx {
            let _ = tx.send(SenderCommand::Cancel);
        }
    }
}

/// Get transfer progress (0.0 - 1.0). Returns -1.0 if session not found.
#[no_mangle]
pub extern "C" fn htp_progress(session_id: u32) -> f64 {
    match sessions().lock().get(&session_id) {
        Some(t) => t.stats.progress(),
        None => -1.0,
    }
}

/// Get transfer stats. Returns false if session not found.
#[no_mangle]
pub extern "C" fn htp_stats(
    session_id: u32,
    out_bytes_transferred: *mut u64,
    out_total_bytes: *mut u64,
    out_rate_bps: *mut u64,
    out_retransmits: *mut u64,
) -> bool {
    match sessions().lock().get(&session_id) {
        Some(t) => {
            unsafe {
                *out_bytes_transferred = t.stats.bytes_transferred.load(Ordering::Relaxed);
                *out_total_bytes = t.stats.total_bytes.load(Ordering::Relaxed);
                *out_rate_bps = t.stats.current_rate.load(Ordering::Relaxed);
                *out_retransmits = t.stats.retransmits.load(Ordering::Relaxed);
            }
            true
        }
        None => false,
    }
}

/// Get the max plaintext bytes per packet.
#[no_mangle]
pub extern "C" fn htp_chunk_size() -> u32 {
    protocol::MAX_PLAINTEXT_PER_PACKET as u32
}

/// Generate a random 32-byte salt.
#[no_mangle]
pub unsafe extern "C" fn htp_random_salt(out: *mut u8) {
    let salt = crypto::TransferCrypto::random_salt();
    std::ptr::copy_nonoverlapping(salt.as_ptr(), out, 32);
}
