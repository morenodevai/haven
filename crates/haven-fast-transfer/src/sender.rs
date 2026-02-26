/// UDP blast sender: 3-thread pipeline.
///
/// ```text
/// [Reader] ---> [Encryptor] ---> [Blaster]
/// Read 4MB       AES-256-GCM     Slice into 1400B frames
/// from disk      encrypt+SHA256  Blast via UDP to server
///                Reuse cipher!   Cache encrypted chunks for retransmit
/// ```

use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Instant;

use aes_gcm::{Aes256Gcm, KeyInit, Nonce, aead::Aead};
use crossbeam_channel::{bounded, Receiver};
use sha2::{Digest, Sha256};

use crate::logging::{TransferEvent, TransferLog, TransferLogger};
use crate::protocol::*;

/// Transfer state constants.
pub const STATE_IDLE: u8 = 0;
pub const STATE_ENCRYPTING: u8 = 1;
pub const STATE_BLASTING: u8 = 2;
pub const STATE_COMPLETE: u8 = 3;
pub const STATE_ERROR: u8 = 4;
pub const STATE_CANCELLED: u8 = 5;

/// Progress tracking for the sender.
pub struct SenderProgress {
    pub bytes_done: AtomicU64,
    pub bytes_total: AtomicU64,
    pub state: AtomicU8,
    pub cancelled: AtomicU8,
    pub chunks_complete: AtomicU64,
    pub chunks_total: AtomicU64,
    pub retransmits: AtomicU64,
    pub rate_bps: AtomicU64,
    pub last_error: std::sync::Mutex<Option<String>>,
    /// Set after encryption pass: JSON `{"file_sha256":"...","chunk_hashes":[...]}`.
    pub hashes_json: std::sync::Mutex<Option<String>>,
}

impl SenderProgress {
    pub fn new() -> Self {
        Self {
            bytes_done: AtomicU64::new(0),
            bytes_total: AtomicU64::new(0),
            state: AtomicU8::new(STATE_IDLE),
            cancelled: AtomicU8::new(0),
            chunks_complete: AtomicU64::new(0),
            chunks_total: AtomicU64::new(0),
            retransmits: AtomicU64::new(0),
            rate_bps: AtomicU64::new(INITIAL_RATE_BPS),
            last_error: std::sync::Mutex::new(None),
            hashes_json: std::sync::Mutex::new(None),
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed) != 0
    }
}

/// Message from encryptor to blaster.
struct EncryptedChunk {
    chunk_index: u32,
    data: Vec<u8>,
    #[allow(dead_code)]
    sha256: String,
    frame_count: u16,
}

/// NACK message received from remote (fed via control channel).
#[derive(Debug, Clone)]
pub struct NackMessage {
    pub chunk_index: u32,
    pub missing_frames: Vec<u16>,
}

/// Chunk ACK from remote.
#[derive(Debug, Clone)]
pub struct ChunkAckMessage {
    pub chunk_index: u32,
}

/// Configuration for the sender.
pub struct SenderConfig {
    pub file_path: String,
    pub target_addr: SocketAddr,
    pub transfer_id: [u8; 16],
    pub encryption_key: [u8; 32],
    pub logger: Option<Arc<dyn TransferLogger>>,
}

/// Result of a completed send operation.
pub struct SendResult {
    pub file_sha256: String,
    pub chunk_hashes: Vec<String>,
    pub encrypted_size: u64,
    pub chunk_count: u32,
}

/// Run the sender pipeline. Blocks until complete, error, or cancellation.
///
/// `nack_rx` receives NACKs from the control channel (WebSocket).
/// `ack_rx` receives chunk ACKs from the control channel.
pub fn run_sender(
    config: SenderConfig,
    progress: Arc<SenderProgress>,
    nack_rx: Receiver<NackMessage>,
    ack_rx: Receiver<ChunkAckMessage>,
) -> Result<SendResult, String> {
    let file_path = Path::new(&config.file_path);
    let file_size = std::fs::metadata(file_path)
        .map_err(|e| format!("Cannot read file: {}", e))?
        .len();

    let chunk_count = if file_size == 0 {
        1u32
    } else {
        ((file_size as usize + CHUNK_SIZE - 1) / CHUNK_SIZE) as u32
    };

    progress.bytes_total.store(file_size, Ordering::Relaxed);
    progress.chunks_total.store(chunk_count as u64, Ordering::Relaxed);
    progress.state.store(STATE_ENCRYPTING, Ordering::Relaxed);

    // Channels between pipeline stages (bounded for backpressure).
    let (read_tx, read_rx) = bounded::<(u32, Vec<u8>)>(4);
    let (enc_tx, enc_rx) = bounded::<EncryptedChunk>(4);

    let transfer_id = config.transfer_id;
    let key = config.encryption_key;

    // ── Reader thread ──────────────────────────────────────────────────
    let progress_reader = progress.clone();
    let file_path_owned = config.file_path.clone();
    let reader_handle = std::thread::spawn(move || -> Result<(), String> {
        use std::io::Read;
        let mut file = std::fs::File::open(&file_path_owned)
            .map_err(|e| format!("Cannot open file: {}", e))?;

        let mut buf = vec![0u8; CHUNK_SIZE];
        for idx in 0..chunk_count {
            if progress_reader.is_cancelled() {
                return Err("Cancelled".into());
            }
            let remaining = file_size - idx as u64 * CHUNK_SIZE as u64;
            let to_read = (remaining as usize).min(CHUNK_SIZE);

            file.read_exact(&mut buf[..to_read])
                .map_err(|e| format!("Read error at chunk {}: {}", idx, e))?;

            if read_tx.send((idx, buf[..to_read].to_vec())).is_err() {
                return Err("Reader channel closed".into());
            }
        }
        Ok(())
    });

    // ── Encryptor thread ───────────────────────────────────────────────
    let progress_enc = progress.clone();
    let logger_enc = config.logger.clone();
    let encryptor_handle = std::thread::spawn(move || -> Result<(String, Vec<String>, u64), String> {
        let cipher = Aes256Gcm::new_from_slice(&key)
            .map_err(|e| format!("Cipher init failed: {}", e))?;
        let mut full_hasher = Sha256::new();
        let mut chunk_hashes = Vec::with_capacity(chunk_count as usize);
        let mut encrypted_size: u64 = 0;

        for (idx, plaintext) in read_rx {
            if progress_enc.is_cancelled() {
                return Err("Cancelled".into());
            }

            let start = Instant::now();

            // Derive deterministic nonce: SHA-256(key || chunk_index_le)[..12]
            let nonce = derive_chunk_nonce(&key, idx);

            // Encrypt: output = nonce(12) + ciphertext+tag
            let ciphertext = cipher
                .encrypt(Nonce::from_slice(&nonce), plaintext.as_ref())
                .map_err(|e| format!("Encrypt chunk {}: {}", idx, e))?;

            let mut encrypted = Vec::with_capacity(12 + ciphertext.len());
            encrypted.extend_from_slice(&nonce);
            encrypted.extend_from_slice(&ciphertext);

            // Hash the encrypted chunk
            let mut chunk_hasher = Sha256::new();
            chunk_hasher.update(&encrypted);
            let hash = hex::encode(chunk_hasher.finalize());

            full_hasher.update(&encrypted);
            encrypted_size += encrypted.len() as u64;

            let frame_count = frames_for_chunk(encrypted.len());
            let duration_ms = start.elapsed().as_millis() as u64;

            if let Some(ref logger) = logger_enc {
                logger.log(TransferLog {
                    component: "sender",
                    transfer_id,
                    event: TransferEvent::ChunkEncrypted {
                        chunk_idx: idx,
                        size: encrypted.len(),
                        duration_ms,
                    },
                });
            }

            chunk_hashes.push(hash.clone());

            if enc_tx
                .send(EncryptedChunk {
                    chunk_index: idx,
                    data: encrypted,
                    sha256: hash,
                    frame_count,
                })
                .is_err()
            {
                return Err("Encryptor channel closed".into());
            }
        }

        let file_sha256 = hex::encode(full_hasher.finalize());
        Ok((file_sha256, chunk_hashes, encrypted_size))
    });

    // ── Blaster thread ─────────────────────────────────────────────────
    // Creates UDP socket, blasts frames, handles NACKs from cache.
    let progress_blast = progress.clone();
    let logger_blast = config.logger.clone();
    let target_addr = config.target_addr;
    let blaster_handle = std::thread::spawn(move || -> Result<(), String> {
        let socket = create_udp_socket()
            .map_err(|e| format!("UDP socket error: {}", e))?;

        if let Some(ref logger) = logger_blast {
            let local = socket.local_addr().map(|a| a.to_string()).unwrap_or_else(|_| "?".into());
            logger.log(TransferLog {
                component: "sender",
                transfer_id,
                event: TransferEvent::VacuumStarted { bind_addr: format!("local={} target={}", local, target_addr) },
            });
        }

        // Cache of recently encrypted chunks for retransmit.
        let mut cache: HashMap<u32, Vec<u8>> = HashMap::new();
        let mut cache_order: Vec<u32> = Vec::new();

        let mut send_buf = vec![0u8; FRAME_MAX];
        let mut rate_bps = INITIAL_RATE_BPS;
        let mut total_retransmits: u64 = 0;

        // Track which chunks are ACKed
        let mut acked: std::collections::HashSet<u32> = std::collections::HashSet::new();

        for chunk in enc_rx {
            if progress_blast.is_cancelled() {
                return Err("Cancelled".into());
            }

            // First encrypted chunk arriving means we can switch to BLASTING state.
            // But we wait until hashes_json is set (after all chunks encrypted).
            if progress_blast.state.load(Ordering::Relaxed) == STATE_ENCRYPTING
                && chunk.chunk_index == 0
            {
                progress_blast
                    .state
                    .store(STATE_BLASTING, Ordering::Relaxed);
            }

            // Cache the encrypted chunk
            cache.insert(chunk.chunk_index, chunk.data.clone());
            cache_order.push(chunk.chunk_index);
            // Evict old entries
            while cache.len() > SENDER_CACHE_SIZE {
                if let Some(old_idx) = cache_order.first().copied() {
                    // Only evict if it's been ACKed
                    if acked.contains(&old_idx) {
                        cache.remove(&old_idx);
                        cache_order.remove(0);
                    } else {
                        break; // Don't evict un-ACKed chunks
                    }
                }
            }

            // Blast all frames for this chunk
            blast_chunk(
                &socket,
                target_addr,
                &transfer_id,
                chunk.chunk_index,
                &chunk.data,
                chunk.frame_count,
                &mut send_buf,
                rate_bps,
            )?;

            if let Some(ref logger) = logger_blast {
                logger.log(TransferLog {
                    component: "sender",
                    transfer_id,
                    event: TransferEvent::FramesBlasted {
                        chunk_idx: chunk.chunk_index,
                        frame_count: chunk.frame_count,
                    },
                });
            }

            progress_blast
                .bytes_done
                .fetch_add(chunk.data.len() as u64, Ordering::Relaxed);

            // Process any pending NACKs (non-blocking)
            while let Ok(nack) = nack_rx.try_recv() {
                if let Some(cached_data) = cache.get(&nack.chunk_index) {
                    let fc = frames_for_chunk(cached_data.len());
                    retransmit_frames(
                        &socket,
                        target_addr,
                        &transfer_id,
                        nack.chunk_index,
                        cached_data,
                        fc,
                        &nack.missing_frames,
                        &mut send_buf,
                    )?;
                    total_retransmits += nack.missing_frames.len() as u64;
                    progress_blast
                        .retransmits
                        .fetch_add(nack.missing_frames.len() as u64, Ordering::Relaxed);

                    // Rate control: check loss
                    let loss_pct = nack.missing_frames.len() as f64 / fc as f64;
                    let old_rate = rate_bps;
                    if loss_pct > LOSS_THRESHOLD_HIGH {
                        rate_bps = (rate_bps as f64 * RATE_DECREASE) as u64;
                    }
                    if old_rate != rate_bps {
                        progress_blast.rate_bps.store(rate_bps, Ordering::Relaxed);
                        if let Some(ref logger) = logger_blast {
                            logger.log(TransferLog {
                                component: "sender",
                                transfer_id,
                                event: TransferEvent::RateAdjusted {
                                    old_rate_bps: old_rate,
                                    new_rate_bps: rate_bps,
                                    loss_pct,
                                },
                            });
                        }
                    }
                }
            }

            // Process ACKs (non-blocking)
            while let Ok(ack) = ack_rx.try_recv() {
                acked.insert(ack.chunk_index);
                progress_blast
                    .chunks_complete
                    .fetch_add(1, Ordering::Relaxed);

                // Rate increase on successful chunk with low loss
                let old_rate = rate_bps;
                rate_bps = (rate_bps as f64 * RATE_INCREASE).min(INITIAL_RATE_BPS as f64) as u64;
                if old_rate != rate_bps {
                    progress_blast.rate_bps.store(rate_bps, Ordering::Relaxed);
                }
            }
        }

        // All chunks blasted. Now wait for remaining NACKs and ACKs until all chunks ACKed.
        // This loop handles retransmits for the tail end of the transfer.
        let deadline = Instant::now() + std::time::Duration::from_secs(60);
        let chunk_count = cache_order.last().map(|&x| x + 1).unwrap_or(0);

        while acked.len() < chunk_count as usize && Instant::now() < deadline {
            if progress_blast.is_cancelled() {
                return Err("Cancelled".into());
            }

            // Process NACKs with timeout
            if let Ok(nack) = nack_rx.recv_timeout(std::time::Duration::from_millis(100)) {
                if let Some(cached_data) = cache.get(&nack.chunk_index) {
                    let fc = frames_for_chunk(cached_data.len());
                    retransmit_frames(
                        &socket,
                        target_addr,
                        &transfer_id,
                        nack.chunk_index,
                        cached_data,
                        fc,
                        &nack.missing_frames,
                        &mut send_buf,
                    )?;
                    total_retransmits += nack.missing_frames.len() as u64;
                    progress_blast
                        .retransmits
                        .fetch_add(nack.missing_frames.len() as u64, Ordering::Relaxed);
                }
            }

            // Process ACKs
            while let Ok(ack) = ack_rx.try_recv() {
                acked.insert(ack.chunk_index);
                progress_blast
                    .chunks_complete
                    .fetch_add(1, Ordering::Relaxed);
            }
        }

        if let Some(ref logger) = logger_blast {
            logger.log(TransferLog {
                component: "sender",
                transfer_id,
                event: TransferEvent::TransferComplete {
                    total_bytes: progress_blast.bytes_done.load(Ordering::Relaxed),
                    duration_ms: 0, // caller can compute from wall clock
                    retransmits: total_retransmits,
                },
            });
        }

        Ok(())
    });

    // ── Wait for pipeline to complete ──────────────────────────────────
    reader_handle
        .join()
        .map_err(|_| "Reader thread panicked".to_string())??;

    let (file_sha256, chunk_hashes, encrypted_size) = encryptor_handle
        .join()
        .map_err(|_| "Encryptor thread panicked".to_string())??;

    // Store hashes so they can be read before blasting finishes
    {
        let json = format!(
            r#"{{"file_sha256":"{}","chunk_hashes":{}}}"#,
            file_sha256,
            serde_json_mini(&chunk_hashes),
        );
        *progress.hashes_json.lock().unwrap() = Some(json);
    }

    blaster_handle
        .join()
        .map_err(|_| "Blaster thread panicked".to_string())??;

    progress.state.store(STATE_COMPLETE, Ordering::Relaxed);

    Ok(SendResult {
        file_sha256,
        chunk_hashes,
        encrypted_size,
        chunk_count,
    })
}

/// Derive deterministic chunk nonce: SHA-256(key || chunk_index_le)[..12].
fn derive_chunk_nonce(key: &[u8; 32], chunk_index: u32) -> [u8; 12] {
    let mut hasher = Sha256::new();
    hasher.update(key);
    hasher.update(&(chunk_index as u64).to_le_bytes());
    let hash = hasher.finalize();
    let mut nonce = [0u8; 12];
    nonce.copy_from_slice(&hash[..12]);
    nonce
}

/// Blast all frames for one encrypted chunk over UDP.
fn blast_chunk(
    socket: &std::net::UdpSocket,
    target: SocketAddr,
    transfer_id: &[u8; 16],
    chunk_index: u32,
    encrypted_data: &[u8],
    frame_count: u16,
    send_buf: &mut [u8],
    rate_bps: u64,
) -> Result<(), String> {
    // Calculate inter-frame delay for rate limiting.
    // bytes_per_frame ~= 1424, so frames_per_sec = rate_bps / 1424
    let nanos_per_frame = if rate_bps > 0 {
        (FRAME_MAX as u128 * 1_000_000_000) / rate_bps as u128
    } else {
        0
    };

    let mut offset = 0usize;
    for frame_idx in 0..frame_count {
        let end = (offset + FRAME_PAYLOAD).min(encrypted_data.len());
        let payload = &encrypted_data[offset..end];

        let len = encode_frame(
            send_buf,
            transfer_id,
            chunk_index,
            frame_idx,
            frame_count,
            payload,
        );

        // Retry on ENOBUFS / WSAENOBUFS (OS error 10055 on Windows)
        // which means the send buffer is full — back off briefly and retry.
        let mut retries = 0;
        loop {
            match socket.send_to(&send_buf[..len], target) {
                Ok(_) => break,
                Err(ref e) if retries < 50 && (
                    e.kind() == io::ErrorKind::WouldBlock
                    || e.raw_os_error() == Some(10055) // WSAENOBUFS
                    || e.raw_os_error() == Some(105)   // ENOBUFS (Linux)
                ) => {
                    retries += 1;
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                Err(e) => return Err(format!("UDP send error: {}", e)),
            }
        }

        offset = end;

        // Rate limiting via busy-spin for sub-microsecond precision
        if nanos_per_frame > 0 {
            let target_time = std::time::Instant::now()
                + std::time::Duration::from_nanos(nanos_per_frame as u64);
            while std::time::Instant::now() < target_time {
                std::hint::spin_loop();
            }
        }
    }

    Ok(())
}

/// Retransmit specific frames from cached encrypted chunk data.
fn retransmit_frames(
    socket: &std::net::UdpSocket,
    target: SocketAddr,
    transfer_id: &[u8; 16],
    chunk_index: u32,
    encrypted_data: &[u8],
    frame_count: u16,
    missing_frames: &[u16],
    send_buf: &mut [u8],
) -> Result<(), String> {
    for &frame_idx in missing_frames {
        if frame_idx >= frame_count {
            continue;
        }
        let offset = frame_idx as usize * FRAME_PAYLOAD;
        let end = (offset + FRAME_PAYLOAD).min(encrypted_data.len());
        if offset >= encrypted_data.len() {
            continue;
        }
        let payload = &encrypted_data[offset..end];

        let len = encode_frame(
            send_buf,
            transfer_id,
            chunk_index,
            frame_idx,
            frame_count,
            payload,
        );

        let mut retries = 0;
        loop {
            match socket.send_to(&send_buf[..len], target) {
                Ok(_) => break,
                Err(ref e) if retries < 50 && (
                    e.kind() == io::ErrorKind::WouldBlock
                    || e.raw_os_error() == Some(10055)
                    || e.raw_os_error() == Some(105)
                ) => {
                    retries += 1;
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                Err(e) => return Err(format!("UDP retransmit error: {}", e)),
            }
        }
    }
    Ok(())
}

/// Create a UDP socket with appropriate buffer sizes.
fn create_udp_socket() -> io::Result<std::net::UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};

    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_nonblocking(false)?;
    // Bind to any available port
    socket.bind(&"0.0.0.0:0".parse::<SocketAddr>().unwrap().into())?;
    // Set send buffer size
    socket.set_send_buffer_size(UDP_RECV_BUFFER)?;

    Ok(socket.into())
}

/// Configuration for the raw sender (blasts pre-encrypted data from file).
pub struct RawSenderConfig {
    pub file_path: String,
    pub target_addr: SocketAddr,
    pub transfer_id: [u8; 16],
    pub file_size: u64,
    pub chunk_size: u64,
    pub chunk_count: u32,
    pub logger: Option<Arc<dyn TransferLogger>>,
}

/// Run the raw sender pipeline. Reads pre-encrypted data from file and blasts
/// as UDP frames. Used by the file server to send downloads.
///
/// Unlike `run_sender`, this does NOT encrypt — data is already encrypted on disk.
pub fn run_raw_sender(
    config: RawSenderConfig,
    progress: Arc<SenderProgress>,
    nack_rx: Receiver<NackMessage>,
    ack_rx: Receiver<ChunkAckMessage>,
) -> Result<(), String> {
    use std::io::Read;

    progress
        .bytes_total
        .store(config.file_size, Ordering::Relaxed);
    progress
        .chunks_total
        .store(config.chunk_count as u64, Ordering::Relaxed);
    progress.state.store(STATE_BLASTING, Ordering::Relaxed);

    let socket = create_udp_socket().map_err(|e| format!("UDP socket error: {}", e))?;

    let mut file = std::fs::File::open(&config.file_path)
        .map_err(|e| format!("Cannot open file: {}", e))?;

    let mut cache: HashMap<u32, Vec<u8>> = HashMap::new();
    let mut cache_order: Vec<u32> = Vec::new();
    let mut acked: std::collections::HashSet<u32> = std::collections::HashSet::new();
    let mut send_buf = vec![0u8; FRAME_MAX];
    let mut rate_bps = INITIAL_RATE_BPS;
    let transfer_id = config.transfer_id;

    for idx in 0..config.chunk_count {
        if progress.is_cancelled() {
            return Err("Cancelled".into());
        }

        // Calculate this chunk's size
        let remaining = config.file_size - idx as u64 * config.chunk_size;
        let this_chunk_size = remaining.min(config.chunk_size) as usize;

        let mut chunk_data = vec![0u8; this_chunk_size];
        file.read_exact(&mut chunk_data)
            .map_err(|e| format!("Read error at chunk {}: {}", idx, e))?;

        let frame_count = frames_for_chunk(chunk_data.len());

        // Cache for retransmit
        cache.insert(idx, chunk_data.clone());
        cache_order.push(idx);
        while cache.len() > SENDER_CACHE_SIZE {
            if let Some(&old_idx) = cache_order.first() {
                if acked.contains(&old_idx) {
                    cache.remove(&old_idx);
                    cache_order.remove(0);
                } else {
                    break;
                }
            }
        }

        // Blast
        blast_chunk(
            &socket,
            config.target_addr,
            &transfer_id,
            idx,
            &chunk_data,
            frame_count,
            &mut send_buf,
            rate_bps,
        )?;

        if let Some(ref logger) = config.logger {
            logger.log(TransferLog {
                component: "raw_sender",
                transfer_id,
                event: TransferEvent::FramesBlasted {
                    chunk_idx: idx,
                    frame_count,
                },
            });
        }

        progress
            .bytes_done
            .fetch_add(chunk_data.len() as u64, Ordering::Relaxed);

        // Process NACKs
        while let Ok(nack) = nack_rx.try_recv() {
            if let Some(cached) = cache.get(&nack.chunk_index) {
                let fc = frames_for_chunk(cached.len());
                retransmit_frames(
                    &socket,
                    config.target_addr,
                    &transfer_id,
                    nack.chunk_index,
                    cached,
                    fc,
                    &nack.missing_frames,
                    &mut send_buf,
                )?;
                progress
                    .retransmits
                    .fetch_add(nack.missing_frames.len() as u64, Ordering::Relaxed);

                let loss_pct = nack.missing_frames.len() as f64 / fc as f64;
                if loss_pct > LOSS_THRESHOLD_HIGH {
                    rate_bps = (rate_bps as f64 * RATE_DECREASE) as u64;
                    progress.rate_bps.store(rate_bps, Ordering::Relaxed);
                }
            }
        }

        // Process ACKs
        while let Ok(ack) = ack_rx.try_recv() {
            acked.insert(ack.chunk_index);
            progress.chunks_complete.fetch_add(1, Ordering::Relaxed);
            rate_bps = (rate_bps as f64 * RATE_INCREASE).min(INITIAL_RATE_BPS as f64) as u64;
            progress.rate_bps.store(rate_bps, Ordering::Relaxed);
        }
    }

    // Wait for remaining NACKs/ACKs
    let deadline = Instant::now() + std::time::Duration::from_secs(60);
    while acked.len() < config.chunk_count as usize && Instant::now() < deadline {
        if progress.is_cancelled() {
            return Err("Cancelled".into());
        }
        if let Ok(nack) = nack_rx.recv_timeout(std::time::Duration::from_millis(100)) {
            if let Some(cached) = cache.get(&nack.chunk_index) {
                let fc = frames_for_chunk(cached.len());
                retransmit_frames(
                    &socket,
                    config.target_addr,
                    &transfer_id,
                    nack.chunk_index,
                    cached,
                    fc,
                    &nack.missing_frames,
                    &mut send_buf,
                )?;
            }
        }
        while let Ok(ack) = ack_rx.try_recv() {
            acked.insert(ack.chunk_index);
            progress.chunks_complete.fetch_add(1, Ordering::Relaxed);
        }
    }

    progress.state.store(STATE_COMPLETE, Ordering::Relaxed);
    Ok(())
}

/// Minimal JSON array serialization without serde dependency.
fn serde_json_mini(hashes: &[String]) -> String {
    let mut s = String::from("[");
    for (i, h) in hashes.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push('"');
        s.push_str(h);
        s.push('"');
    }
    s.push(']');
    s
}
