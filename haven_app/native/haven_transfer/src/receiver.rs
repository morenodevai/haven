/// Receiver: authenticates with relay, sends hello, receives/decrypts/writes.
///
/// Flow:
///   1. Bind UDP socket
///   2. Authenticate with relay (JWT handshake on this socket)
///   3. Send a "hello" HTP packet so the relay learns our address for this session
///   4. Receive packets, decrypt, write to correct offset in output file
///   5. Periodically send NACKs for missing sequences via the event channel
///   6. When all chunks received → signal DONE

use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::Sender as ChannelSender;
use parking_lot::Mutex;

use crate::crypto::TransferCrypto;
use crate::protocol::*;
use crate::stats::TransferStats;

/// How often to check for gaps and send NACKs.
const NACK_INTERVAL: Duration = Duration::from_millis(100);

/// Maximum number of sequences to report in a single NACK.
const MAX_NACK_SIZE: usize = 500;

/// Messages from receiver to control channel.
pub enum ReceiverEvent {
    /// These sequences are missing — tell the sender.
    Nack(Vec<u64>),
    /// Transfer complete — all chunks received.
    Done { total_bytes: u64 },
    /// Progress update.
    Progress { bytes_received: u64, chunks_received: u64 },
}

/// Receiver configuration.
pub struct RecvConfig {
    pub session_id: u32,
    pub output_path: String,
    pub file_size: u64,
    pub total_chunks: u64,
    pub listen_addr: SocketAddr,
    pub master_key: [u8; 32],
    pub salt: [u8; 32],
    /// JWT token for authenticating with the UDP relay. Empty = skip (loopback).
    pub jwt_token: String,
    /// Relay address to authenticate with and send hello to.
    /// Required when jwt_token is non-empty.
    pub relay_addr: Option<SocketAddr>,
}

/// Run the receiver. Blocks until transfer completes or is cancelled.
pub fn run_receiver(
    config: RecvConfig,
    event_tx: ChannelSender<ReceiverEvent>,
    stats: Arc<TransferStats>,
    cancelled: Arc<AtomicBool>,
) -> Result<RecvResult, RecvError> {
    // Create/open output file and pre-allocate
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&config.output_path)
        .map_err(RecvError::Io)?;
    file.set_len(config.file_size).map_err(RecvError::Io)?;

    // Setup crypto
    let crypto = TransferCrypto::new(&config.master_key, &config.salt);

    // Bind UDP socket
    let socket = UdpSocket::bind(config.listen_addr).map_err(RecvError::Io)?;

    // Increase receive buffer
    let _ = set_recv_buffer(&socket, 4 * 1024 * 1024);

    // Authenticate with the relay (if token provided)
    if !config.jwt_token.is_empty() {
        if let Some(relay_addr) = config.relay_addr {
            authenticate_udp(&socket, relay_addr, &config.jwt_token)?;

            // Send a "hello" HTP packet to the relay so it registers us
            // for this session. This is an empty HTP packet (header only,
            // sequence=u64::MAX as a sentinel that won't collide with real data).
            let hello = PacketHeader {
                session_id: config.session_id,
                sequence: u64::MAX, // sentinel — sender will never use this sequence
                flags: 0,
            };
            let mut hello_buf = [0u8; HEADER_SIZE];
            hello.write_to(&mut hello_buf);
            let _ = socket.send_to(&hello_buf, relay_addr);
            log::info!("Sent hello packet for session {} to relay", config.session_id);
        }
    }

    socket
        .set_read_timeout(Some(Duration::from_millis(50)))
        .map_err(RecvError::Io)?;

    // Tracking state
    let received: Arc<Mutex<HashSet<u64>>> = Arc::new(Mutex::new(HashSet::new()));
    let mut recv_buf = vec![0u8; MAX_UDP_PAYLOAD + 64];
    let mut bytes_written: u64 = 0;
    let mut last_nack_time = Instant::now();
    let start = Instant::now();

    stats.set_total(config.file_size, config.total_chunks);

    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err(RecvError::Cancelled);
        }

        // Check if transfer complete
        {
            let rcvd = received.lock();
            if rcvd.len() as u64 >= config.total_chunks {
                file.sync_all().map_err(RecvError::Io)?;
                let _ = event_tx.send(ReceiverEvent::Done {
                    total_bytes: config.file_size,
                });
                let elapsed = start.elapsed();
                return Ok(RecvResult {
                    total_bytes: config.file_size,
                    elapsed,
                    throughput_bps: (config.file_size as f64 / elapsed.as_secs_f64()) as u64,
                });
            }
        }

        // Receive a packet
        match socket.recv_from(&mut recv_buf) {
            Ok((len, _src)) => {
                if let Some(packet) = DataPacket::from_bytes(&recv_buf[..len]) {
                    if packet.header.session_id != config.session_id {
                        continue;
                    }

                    let seq = packet.header.sequence;

                    // Skip the hello sentinel
                    if seq == u64::MAX {
                        continue;
                    }

                    // Skip duplicates
                    {
                        let rcvd = received.lock();
                        if rcvd.contains(&seq) {
                            continue;
                        }
                    }

                    // Decrypt
                    match crypto.decrypt(
                        config.session_id,
                        seq,
                        &packet.encrypted_payload,
                    ) {
                        Ok(plaintext) => {
                            let offset = seq * MAX_PLAINTEXT_PER_PACKET as u64;
                            file.seek(SeekFrom::Start(offset)).map_err(RecvError::Io)?;
                            file.write_all(&plaintext).map_err(RecvError::Io)?;

                            bytes_written += plaintext.len() as u64;

                            let chunk_count = {
                                let mut rcvd = received.lock();
                                rcvd.insert(seq);
                                rcvd.len() as u64
                            };

                            stats.update(bytes_written, chunk_count, 0, 0);

                            if chunk_count % 100 == 0 {
                                let _ = event_tx.send(ReceiverEvent::Progress {
                                    bytes_received: bytes_written,
                                    chunks_received: chunk_count,
                                });
                            }
                        }
                        Err(e) => {
                            log::warn!("Decrypt failed for seq {}: {}", seq, e);
                        }
                    }
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => {
                return Err(RecvError::Io(e));
            }
        }

        // Periodic NACK
        if last_nack_time.elapsed() >= NACK_INTERVAL {
            last_nack_time = Instant::now();

            let rcvd = received.lock();
            if (rcvd.len() as u64) < config.total_chunks {
                let mut missing = Vec::new();
                let max_received = rcvd.iter().max().copied().unwrap_or(0);
                let scan_end = (max_received + 100).min(config.total_chunks);
                for seq in 0..scan_end {
                    if !rcvd.contains(&seq) {
                        missing.push(seq);
                        if missing.len() >= MAX_NACK_SIZE { break; }
                    }
                }
                drop(rcvd);

                if !missing.is_empty() {
                    let _ = event_tx.send(ReceiverEvent::Nack(missing));
                }
            }
        }
    }
}

/// Authenticate with the UDP relay server.
fn authenticate_udp(
    socket: &UdpSocket,
    relay_addr: SocketAddr,
    jwt_token: &str,
) -> Result<(), RecvError> {
    let jwt_bytes = jwt_token.as_bytes();
    let mut handshake = Vec::with_capacity(3 + jwt_bytes.len());
    handshake.push(0x00);
    handshake.push((jwt_bytes.len() >> 8) as u8);
    handshake.push(jwt_bytes.len() as u8);
    handshake.extend_from_slice(jwt_bytes);

    socket.send_to(&handshake, relay_addr).map_err(RecvError::Io)?;

    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(RecvError::Io)?;

    let mut resp = [0u8; 2];
    match socket.recv_from(&mut resp) {
        Ok((len, _)) => {
            if len >= 2 && resp[0] == 0x00 && resp[1] == 0x01 {
                log::info!("UDP relay authentication successful (receiver)");
                Ok(())
            } else {
                log::error!("UDP relay authentication rejected (receiver)");
                Err(RecvError::AuthFailed)
            }
        }
        Err(e) => {
            log::error!("UDP relay authentication timeout (receiver): {}", e);
            Err(RecvError::AuthFailed)
        }
    }
}

pub struct RecvResult {
    pub total_bytes: u64,
    pub elapsed: Duration,
    pub throughput_bps: u64,
}

/// Set the UDP receive buffer size.
fn set_recv_buffer(socket: &UdpSocket, size: usize) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::io::{AsRawSocket, FromRawSocket};
        use socket2::Socket;
        let raw = socket.as_raw_socket();
        let s2 = unsafe { Socket::from_raw_socket(raw) };
        let result = s2.set_recv_buffer_size(size);
        std::mem::forget(s2);
        result
    }
    #[cfg(unix)]
    {
        use std::os::unix::io::{AsRawFd, FromRawFd};
        use socket2::Socket;
        let raw = socket.as_raw_fd();
        let s2 = unsafe { Socket::from_raw_fd(raw) };
        let result = s2.set_recv_buffer_size(size);
        std::mem::forget(s2);
        result
    }
}

#[derive(Debug)]
pub enum RecvError {
    Io(std::io::Error),
    Cancelled,
    AuthFailed,
}

impl std::fmt::Display for RecvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RecvError::Io(e) => write!(f, "I/O error: {}", e),
            RecvError::Cancelled => write!(f, "transfer cancelled"),
            RecvError::AuthFailed => write!(f, "UDP relay authentication failed"),
        }
    }
}
