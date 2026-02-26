/// UDP blast receiver: 3-thread pipeline.
///
/// ```text
/// [UDP Vacuum] ---> [Assembler] ---> [Writer]
/// recv_from()        Bitfield         Verify SHA-256
/// Into ring buf      per chunk        Write 4MB to disk
/// 32MB recv buf      NACK missing     at chunk offset
/// ```
///
/// Used by both the file server (receiving uploads) and the download client.

use std::io;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Instant;

use crossbeam_channel::bounded;
use sha2::{Digest, Sha256};

use crate::bitfield::ChunkBitfield;
use crate::logging::{TransferEvent, TransferLog, TransferLogger};
use crate::protocol::*;

/// Receiver progress tracking.
pub struct ReceiverProgress {
    pub bytes_done: AtomicU64,
    pub bytes_total: AtomicU64,
    pub state: AtomicU8,
    pub cancelled: AtomicU8,
    pub chunks_complete: AtomicU64,
    pub chunks_total: AtomicU64,
    pub retransmits: AtomicU64,
    pub rate_bps: AtomicU64,
    pub last_error: std::sync::Mutex<Option<String>>,
}

/// Receiver state constants (same as sender for consistency).
pub const STATE_IDLE: u8 = 0;
pub const STATE_RECEIVING: u8 = 2;
pub const STATE_COMPLETE: u8 = 3;
pub const STATE_ERROR: u8 = 4;
pub const STATE_CANCELLED: u8 = 5;

impl ReceiverProgress {
    pub fn new() -> Self {
        Self {
            bytes_done: AtomicU64::new(0),
            bytes_total: AtomicU64::new(0),
            state: AtomicU8::new(STATE_IDLE),
            cancelled: AtomicU8::new(0),
            chunks_complete: AtomicU64::new(0),
            chunks_total: AtomicU64::new(0),
            retransmits: AtomicU64::new(0),
            rate_bps: AtomicU64::new(0),
            last_error: std::sync::Mutex::new(None),
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed) != 0
    }
}

/// Configuration for the receiver.
pub struct ReceiverConfig {
    pub output_path: String,
    pub transfer_id: [u8; 16],
    pub file_size: u64,
    pub chunk_count: u32,
    pub chunk_size: u64,
    pub chunk_hashes: Vec<String>,
    pub file_sha256: String,
    pub bind_addr: SocketAddr,
    pub logger: Option<Arc<dyn TransferLogger>>,
    /// Optional pre-bound UDP socket. If provided, the receiver uses this socket
    /// instead of creating a new one. This avoids port race conditions when the
    /// caller needs to know the bound port before starting the receiver.
    pub pre_bound_socket: Option<std::net::UdpSocket>,
}

/// Internal message from assembler to writer.
struct AssembledChunk {
    chunk_index: u32,
    data: Vec<u8>,
}

/// NACK callback: called by assembler when frames are missing.
/// The caller should send these over WebSocket to the sender/server.
pub type NackCallback = Box<dyn Fn(u32, Vec<u16>) + Send + Sync>;

/// Run the receiver pipeline. Blocks until complete, error, or cancellation.
///
/// `nack_callback` is called when the assembler detects missing frames.
/// The callback should send a FastNack over WebSocket.
///
/// Returns the bound UDP address (for the caller to communicate back).
pub fn run_receiver(
    config: ReceiverConfig,
    progress: Arc<ReceiverProgress>,
    nack_callback: NackCallback,
) -> Result<SocketAddr, String> {
    let file_size = config.file_size;
    let chunk_count = config.chunk_count;

    progress.bytes_total.store(file_size, Ordering::Relaxed);
    progress
        .chunks_total
        .store(chunk_count as u64, Ordering::Relaxed);
    progress.state.store(STATE_RECEIVING, Ordering::Relaxed);

    // Pre-allocate output file
    {
        let path = Path::new(&config.output_path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Cannot create output dir: {}", e))?;
        }
        let file = std::fs::File::create(path)
            .map_err(|e| format!("Cannot create output file: {}", e))?;
        file.set_len(file_size)
            .map_err(|e| format!("Cannot allocate output file: {}", e))?;
    }

    // Create UDP socket (or use pre-bound one)
    let socket = match config.pre_bound_socket {
        Some(s) => {
            // Configure the pre-bound socket for receiving
            s.set_nonblocking(false).map_err(|e| format!("Socket config: {}", e))?;
            s.set_read_timeout(Some(std::time::Duration::from_millis(100)))
                .map_err(|e| format!("Socket timeout: {}", e))?;
            // Recv buffer should already be set by caller
            s
        }
        None => create_recv_socket(config.bind_addr)
            .map_err(|e| format!("UDP bind error: {}", e))?,
    };
    let bound_addr = socket
        .local_addr()
        .map_err(|e| format!("Cannot get bound addr: {}", e))?;

    // Channels
    let (frame_tx, frame_rx) = bounded::<(FrameHeader, Vec<u8>)>(RING_BUFFER_FRAMES);
    let (assembled_tx, assembled_rx) = bounded::<AssembledChunk>(4);

    let transfer_id = config.transfer_id;

    // ── Thread 1: UDP Vacuum ───────────────────────────────────────────
    let progress_vacuum = progress.clone();
    let logger_vacuum = config.logger.clone();
    let vacuum_handle = std::thread::spawn(move || -> Result<(), String> {
        let mut recv_buf = vec![0u8; FRAME_MAX + 64]; // extra safety margin
        let mut frames_received: u64 = 0;
        let mut frames_rejected: u64 = 0;
        let local_addr = socket.local_addr().map(|a| a.to_string()).unwrap_or_else(|_| "?".into());

        if let Some(ref logger) = logger_vacuum {
            logger.log(TransferLog {
                component: "receiver",
                transfer_id,
                event: TransferEvent::VacuumStarted { bind_addr: local_addr.clone() },
            });
        }

        loop {
            if progress_vacuum.is_cancelled() {
                return Err("Cancelled".into());
            }

            // Check if we're done
            if progress_vacuum.state.load(Ordering::Relaxed) == STATE_COMPLETE {
                return Ok(());
            }

            match socket.recv_from(&mut recv_buf) {
                Ok((len, src)) => {
                    if len < FRAME_HEADER {
                        continue;
                    }

                    if let Some(header) = decode_frame_header(&recv_buf[..len]) {
                        // Verify transfer ID
                        if header.transfer_id != transfer_id {
                            frames_rejected += 1;
                            if frames_rejected <= 3 {
                                if let Some(ref logger) = logger_vacuum {
                                    logger.log(TransferLog {
                                        component: "receiver",
                                        transfer_id,
                                        event: TransferEvent::TransferIdMismatch {
                                            got: header.transfer_id,
                                            from: src.to_string(),
                                        },
                                    });
                                }
                            }
                            continue;
                        }

                        frames_received += 1;
                        if frames_received == 1 || frames_received % 10000 == 0 {
                            if let Some(ref logger) = logger_vacuum {
                                logger.log(TransferLog {
                                    component: "receiver",
                                    transfer_id,
                                    event: TransferEvent::VacuumProgress {
                                        frames_received,
                                        from: src.to_string(),
                                    },
                                });
                            }
                        }

                        let payload = recv_buf[FRAME_HEADER..len].to_vec();
                        if frame_tx.send((header, payload)).is_err() {
                            return Ok(()); // Channel closed, assembler done
                        }
                    }
                }
                Err(ref e)
                    if e.kind() == io::ErrorKind::WouldBlock
                        || e.kind() == io::ErrorKind::TimedOut =>
                {
                    // Timeout on recv — check cancellation and continue
                    // Note: Windows returns TimedOut, Unix returns WouldBlock
                    continue;
                }
                Err(e) => {
                    return Err(format!("UDP recv error: {}", e));
                }
            }
        }
    });

    // ── Thread 2: Assembler ────────────────────────────────────────────
    let progress_asm = progress.clone();
    let logger_asm = config.logger.clone();
    let _chunk_hashes = config.chunk_hashes.clone();
    let chunk_size = config.chunk_size;
    let nack_cb = Arc::new(nack_callback);

    let assembler_handle = std::thread::spawn(move || -> Result<(), String> {
        // Per-chunk assembly state
        let mut bitfields: Vec<Option<ChunkBitfield>> = vec![None; chunk_count as usize];
        let mut buffers: Vec<Option<Vec<u8>>> = vec![None; chunk_count as usize];
        let mut completed = vec![false; chunk_count as usize];
        let mut completed_count = 0u32;

        let mut last_nack_scan = Instant::now();

        loop {
            if progress_asm.is_cancelled() {
                return Err("Cancelled".into());
            }

            if completed_count >= chunk_count {
                // All chunks assembled
                progress_asm.state.store(STATE_COMPLETE, Ordering::Relaxed);
                return Ok(());
            }

            // Try to receive a frame with timeout for NACK scanning
            match frame_rx.recv_timeout(std::time::Duration::from_millis(NACK_SCAN_INTERVAL_MS)) {
                Ok((header, payload)) => {
                    let cidx = header.chunk_index as usize;
                    if cidx >= chunk_count as usize || completed[cidx] {
                        continue;
                    }

                    // Initialize bitfield and buffer on first frame for this chunk
                    if bitfields[cidx].is_none() {
                        bitfields[cidx] = Some(ChunkBitfield::new(header.frame_count));
                        // Calculate expected chunk size.
                        // file_size and chunk_size are both encrypted sizes.
                        let this_chunk_size = if cidx == chunk_count as usize - 1 {
                            (file_size - cidx as u64 * chunk_size) as usize
                        } else {
                            chunk_size as usize
                        };
                        buffers[cidx] = Some(vec![0u8; this_chunk_size]);
                    }

                    let bf = bitfields[cidx].as_mut().unwrap();
                    let buf = buffers[cidx].as_mut().unwrap();

                    // Copy payload into buffer at frame_index * FRAME_PAYLOAD
                    if bf.set(header.frame_index) {
                        let offset = header.frame_index as usize * FRAME_PAYLOAD;
                        let end = (offset + payload.len()).min(buf.len());
                        let copy_len = end - offset;
                        buf[offset..offset + copy_len].copy_from_slice(&payload[..copy_len]);
                    }

                    // Check if chunk is complete
                    if bf.is_complete() {
                        completed[cidx] = true;
                        completed_count += 1;

                        let data = buffers[cidx].take().unwrap();
                        bitfields[cidx] = None;

                        if assembled_tx
                            .send(AssembledChunk {
                                chunk_index: header.chunk_index,
                                data,
                            })
                            .is_err()
                        {
                            return Err("Assembled channel closed".into());
                        }
                    }
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    // NACK scan timeout — fall through to scan below
                }
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    return Ok(());
                }
            }

            // Periodic NACK scan
            if last_nack_scan.elapsed().as_millis() >= NACK_SCAN_INTERVAL_MS as u128 {
                last_nack_scan = Instant::now();

                for cidx in 0..chunk_count as usize {
                    if completed[cidx] {
                        continue;
                    }
                    if let Some(ref bf) = bitfields[cidx] {
                        if bf.received() > 0 && !bf.is_complete() {
                            let missing = bf.missing_frames();
                            if !missing.is_empty() {
                                progress_asm
                                    .retransmits
                                    .fetch_add(missing.len() as u64, Ordering::Relaxed);

                                if let Some(ref logger) = logger_asm {
                                    logger.log(TransferLog {
                                        component: "receiver",
                                        transfer_id,
                                        event: TransferEvent::NackSent {
                                            chunk_idx: cidx as u32,
                                            missing_count: missing.len() as u16,
                                        },
                                    });
                                }

                                nack_cb(cidx as u32, missing);
                            }
                        }
                    }
                }
            }
        }
    });

    // ── Thread 3: Writer ───────────────────────────────────────────────
    let progress_writer = progress.clone();
    let logger_writer = config.logger.clone();
    let output_path = config.output_path.clone();
    let chunk_hashes_w = config.chunk_hashes.clone();
    let _file_sha256_expected = config.file_sha256.clone();

    let writer_handle = std::thread::spawn(move || -> Result<(), String> {
        use std::io::{Seek, SeekFrom, Write};
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .open(&output_path)
            .map_err(|e| format!("Cannot open output file: {}", e))?;

        let mut chunks_written = 0u32;

        for assembled in assembled_rx {
            if progress_writer.is_cancelled() {
                return Err("Cancelled".into());
            }

            let start = Instant::now();
            let cidx = assembled.chunk_index as usize;

            // Verify SHA-256
            let mut hasher = Sha256::new();
            hasher.update(&assembled.data);
            let actual_hash = hex::encode(hasher.finalize());

            let hash_match = if cidx < chunk_hashes_w.len() {
                actual_hash == chunk_hashes_w[cidx]
            } else {
                false
            };

            if let Some(ref logger) = logger_writer {
                logger.log(TransferLog {
                    component: "receiver",
                    transfer_id,
                    event: TransferEvent::ChunkAssembled {
                        chunk_idx: assembled.chunk_index,
                        hash_match,
                    },
                });
            }

            if !hash_match {
                return Err(format!(
                    "Chunk {} hash mismatch: expected {}, got {}",
                    cidx,
                    chunk_hashes_w.get(cidx).unwrap_or(&String::new()),
                    actual_hash,
                ));
            }

            // Write at chunk offset
            let offset = cidx as u64 * chunk_size;
            file.seek(SeekFrom::Start(offset))
                .map_err(|e| format!("Seek error: {}", e))?;
            file.write_all(&assembled.data)
                .map_err(|e| format!("Write error: {}", e))?;

            let duration_ms = start.elapsed().as_millis() as u64;
            if let Some(ref logger) = logger_writer {
                logger.log(TransferLog {
                    component: "receiver",
                    transfer_id,
                    event: TransferEvent::ChunkWritten {
                        chunk_idx: assembled.chunk_index,
                        duration_ms,
                    },
                });
            }

            progress_writer
                .bytes_done
                .fetch_add(assembled.data.len() as u64, Ordering::Relaxed);
            progress_writer.chunks_complete.fetch_add(1, Ordering::Relaxed);
            chunks_written += 1;

            if chunks_written >= chunk_count {
                break;
            }
        }

        progress_writer.state.store(STATE_COMPLETE, Ordering::Relaxed);
        Ok(())
    });

    // Wait for all threads
    // Vacuum might still be running when assembler completes; signal it via progress state.
    assembler_handle
        .join()
        .map_err(|_| "Assembler thread panicked".to_string())??;

    // Signal vacuum to stop
    progress.state.store(STATE_COMPLETE, Ordering::Relaxed);

    writer_handle
        .join()
        .map_err(|_| "Writer thread panicked".to_string())??;

    // Don't wait for vacuum — it will exit on next recv timeout when it sees STATE_COMPLETE.
    // We can't join it because recv_from might block. Drop it instead.
    drop(vacuum_handle);

    Ok(bound_addr)
}

/// Create a UDP socket bound to the given address with large recv buffer.
fn create_recv_socket(addr: SocketAddr) -> io::Result<std::net::UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};

    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_nonblocking(false)?;
    socket.set_recv_buffer_size(UDP_RECV_BUFFER)?;
    // Set recv timeout so vacuum thread can check cancellation periodically
    socket.set_read_timeout(Some(std::time::Duration::from_millis(100)))?;
    socket.bind(&addr.into())?;

    Ok(socket.into())
}
