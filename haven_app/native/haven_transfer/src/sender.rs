/// Sender: authenticates with relay, reads a file, chunks it, encrypts, paces UDP packets.
///
/// Flow:
///   1. Bind UDP socket
///   2. Authenticate with relay (JWT handshake on this socket)
///   3. Open file, compute total chunks
///   4. Send loop: read → encrypt → pace → send
///   5. Handle NACKs: retransmit missing sequences
///   6. When all chunks ACKed → DONE

use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;
use parking_lot::Mutex;

use crate::congestion::{CongestionConfig, CongestionController};
use crate::crypto::TransferCrypto;
use crate::protocol::*;
use crate::stats::TransferStats;

/// Commands sent to the sender from the control channel.
pub enum SenderCommand {
    /// Receiver reports these sequences are missing — retransmit them.
    Nack(Vec<u64>),
    /// RTT measurement from control channel round-trip.
    RttSample(Duration),
    /// Receiver confirmed receipt of all packets up to this sequence.
    AckUpTo(u64),
    /// Cancel the transfer.
    Cancel,
}

/// Transfer configuration.
pub struct SendConfig {
    pub session_id: u32,
    pub file_path: String,
    pub dest_addr: SocketAddr,
    pub master_key: [u8; 32],
    pub salt: [u8; 32],
    pub congestion: CongestionConfig,
    /// JWT token for authenticating with the UDP relay. Empty = skip auth (loopback).
    pub jwt_token: String,
}

/// Transfer result.
pub struct SendResult {
    pub total_bytes: u64,
    pub total_packets: u64,
    pub retransmits: u64,
    pub elapsed: Duration,
    pub throughput_bps: u64,
}

/// Run the sender. This blocks until the transfer completes or is cancelled.
pub fn run_sender(
    config: SendConfig,
    command_rx: Receiver<SenderCommand>,
    stats: Arc<TransferStats>,
    cancelled: Arc<AtomicBool>,
) -> Result<SendResult, SendError> {
    // Open file
    let mut file = File::open(&config.file_path).map_err(SendError::Io)?;
    let file_size = file.metadata().map_err(SendError::Io)?.len();
    let total_chunks = (file_size + MAX_PLAINTEXT_PER_PACKET as u64 - 1) / MAX_PLAINTEXT_PER_PACKET as u64;

    stats.set_total(file_size, total_chunks);

    // Setup crypto
    let crypto = TransferCrypto::new(&config.master_key, &config.salt);

    // Setup UDP socket
    let socket = UdpSocket::bind("0.0.0.0:0").map_err(SendError::Io)?;

    // Increase send buffer
    let _ = set_send_buffer(&socket, 4 * 1024 * 1024);

    // Authenticate with the relay (if token provided)
    if !config.jwt_token.is_empty() {
        authenticate_udp(&socket, config.dest_addr, &config.jwt_token)?;
    }

    socket.set_nonblocking(false).map_err(SendError::Io)?;

    // Setup congestion controller
    let cc = Arc::new(Mutex::new(CongestionController::new(config.congestion)));

    // Track which sequences need retransmission
    let pending_nacks: Arc<Mutex<HashSet<u64>>> = Arc::new(Mutex::new(HashSet::new()));

    // Pre-allocate buffers
    let mut read_buf = vec![0u8; MAX_PLAINTEXT_PER_PACKET];
    let mut packet_buf = vec![0u8; MAX_UDP_PAYLOAD];

    let start = Instant::now();
    let mut sequence: u64 = 0;
    let mut bytes_sent: u64 = 0;
    let mut retransmit_count: u64 = 0;

    // Phase 1: Send all chunks
    while sequence < total_chunks {
        if cancelled.load(Ordering::Relaxed) {
            return Err(SendError::Cancelled);
        }

        // Process incoming commands (non-blocking)
        while let Ok(cmd) = command_rx.try_recv() {
            match cmd {
                SenderCommand::Nack(seqs) => {
                    pending_nacks.lock().extend(seqs);
                }
                SenderCommand::RttSample(rtt) => {
                    cc.lock().on_rtt_sample(rtt);
                }
                SenderCommand::AckUpTo(seq) => {
                    let bytes = seq * MAX_PLAINTEXT_PER_PACKET as u64;
                    cc.lock().on_ack(bytes.min(file_size));
                }
                SenderCommand::Cancel => {
                    cancelled.store(true, Ordering::Relaxed);
                    return Err(SendError::Cancelled);
                }
            }
        }

        // Priority: retransmit NACKed packets before sending new ones
        let nack_seq = {
            let mut nacks = pending_nacks.lock();
            if let Some(&seq) = nacks.iter().next() {
                nacks.remove(&seq);
                Some(seq)
            } else {
                None
            }
        };

        let (seq_to_send, flags, is_retransmit) = if let Some(nseq) = nack_seq {
            let mut f = FLAG_RETRANSMIT;
            if nseq == 0 { f |= FLAG_START; }
            if nseq == total_chunks - 1 { f |= FLAG_END; }
            (nseq, f, true)
        } else {
            let mut f = 0u16;
            if sequence == 0 { f |= FLAG_START; }
            if sequence == total_chunks - 1 { f |= FLAG_END; }
            (sequence, f, false)
        };

        // Read the chunk from file
        let offset = seq_to_send * MAX_PLAINTEXT_PER_PACKET as u64;
        let remaining = file_size.saturating_sub(offset);
        let chunk_size = (remaining as usize).min(MAX_PLAINTEXT_PER_PACKET);

        if chunk_size == 0 {
            if !is_retransmit { sequence += 1; }
            continue;
        }

        file.seek(SeekFrom::Start(offset)).map_err(SendError::Io)?;
        file.read_exact(&mut read_buf[..chunk_size]).map_err(SendError::Io)?;

        // Encrypt
        let encrypted = crypto
            .encrypt(config.session_id, seq_to_send, &read_buf[..chunk_size])
            .map_err(|_| SendError::Crypto)?;

        // Build packet
        let header = PacketHeader {
            session_id: config.session_id,
            sequence: seq_to_send,
            flags,
        };
        let total_packet_size = HEADER_SIZE + encrypted.len();
        if total_packet_size > packet_buf.len() {
            packet_buf.resize(total_packet_size, 0);
        }
        header.write_to(&mut packet_buf);
        packet_buf[HEADER_SIZE..HEADER_SIZE + encrypted.len()].copy_from_slice(&encrypted);

        // Pace
        let interval = cc.lock().packet_interval(total_packet_size);
        if interval > Duration::from_micros(1) {
            spin_sleep(interval);
        }

        // Send
        socket
            .send_to(&packet_buf[..total_packet_size], config.dest_addr)
            .map_err(SendError::Io)?;

        bytes_sent += chunk_size as u64;
        if is_retransmit {
            retransmit_count += 1;
        } else {
            sequence += 1;
        }

        stats.update(bytes_sent, sequence, retransmit_count, cc.lock().rate());
        cc.lock().update_rate();
    }

    // Phase 2: Drain remaining NACKs
    let drain_deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if Instant::now() > drain_deadline { break; }
        if cancelled.load(Ordering::Relaxed) {
            return Err(SendError::Cancelled);
        }

        match command_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(SenderCommand::Nack(seqs)) => {
                for seq in seqs {
                    if seq >= total_chunks { continue; }
                    let offset = seq * MAX_PLAINTEXT_PER_PACKET as u64;
                    let remaining = file_size.saturating_sub(offset);
                    let chunk_size = (remaining as usize).min(MAX_PLAINTEXT_PER_PACKET);
                    if chunk_size == 0 { continue; }

                    file.seek(SeekFrom::Start(offset)).map_err(SendError::Io)?;
                    file.read_exact(&mut read_buf[..chunk_size]).map_err(SendError::Io)?;

                    let encrypted = crypto
                        .encrypt(config.session_id, seq, &read_buf[..chunk_size])
                        .map_err(|_| SendError::Crypto)?;

                    let header = PacketHeader {
                        session_id: config.session_id,
                        sequence: seq,
                        flags: FLAG_RETRANSMIT,
                    };
                    let total_size = HEADER_SIZE + encrypted.len();
                    header.write_to(&mut packet_buf);
                    packet_buf[HEADER_SIZE..HEADER_SIZE + encrypted.len()]
                        .copy_from_slice(&encrypted);

                    socket
                        .send_to(&packet_buf[..total_size], config.dest_addr)
                        .map_err(SendError::Io)?;
                    retransmit_count += 1;
                }
            }
            Ok(SenderCommand::RttSample(rtt)) => { cc.lock().on_rtt_sample(rtt); }
            Ok(SenderCommand::AckUpTo(seq)) => {
                if seq >= total_chunks { break; }
            }
            Ok(SenderCommand::Cancel) => { return Err(SendError::Cancelled); }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                if pending_nacks.lock().is_empty() { break; }
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }
    }

    let elapsed = start.elapsed();
    let throughput = if elapsed.as_secs_f64() > 0.0 {
        (file_size as f64 / elapsed.as_secs_f64()) as u64
    } else { 0 };

    Ok(SendResult {
        total_bytes: file_size,
        total_packets: total_chunks,
        retransmits: retransmit_count,
        elapsed,
        throughput_bps: throughput,
    })
}

/// Authenticate with the UDP relay server.
/// Sends: [0x00][jwt_len(2 BE)][jwt_bytes]
/// Expects: [0x00][0x01] (OK)
/// Blocks up to 5 seconds.
fn authenticate_udp(
    socket: &UdpSocket,
    relay_addr: SocketAddr,
    jwt_token: &str,
) -> Result<(), SendError> {
    let jwt_bytes = jwt_token.as_bytes();
    let mut handshake = Vec::with_capacity(3 + jwt_bytes.len());
    handshake.push(0x00); // auth marker
    handshake.push((jwt_bytes.len() >> 8) as u8);
    handshake.push(jwt_bytes.len() as u8);
    handshake.extend_from_slice(jwt_bytes);

    socket
        .send_to(&handshake, relay_addr)
        .map_err(SendError::Io)?;

    // Wait for response
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(SendError::Io)?;

    let mut resp = [0u8; 2];
    match socket.recv_from(&mut resp) {
        Ok((len, _)) => {
            if len >= 2 && resp[0] == 0x00 && resp[1] == 0x01 {
                log::info!("UDP relay authentication successful");
                Ok(())
            } else {
                log::error!("UDP relay authentication rejected");
                Err(SendError::AuthFailed)
            }
        }
        Err(e) => {
            log::error!("UDP relay authentication timeout: {}", e);
            Err(SendError::AuthFailed)
        }
    }
}

/// High-resolution sleep using spin loop for sub-millisecond accuracy.
fn spin_sleep(duration: Duration) {
    let target = Instant::now() + duration;
    if duration > Duration::from_millis(1) {
        std::thread::sleep(duration - Duration::from_micros(500));
    }
    while Instant::now() < target {
        std::hint::spin_loop();
    }
}

/// Set the UDP send buffer size.
fn set_send_buffer(socket: &UdpSocket, size: usize) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::io::{AsRawSocket, FromRawSocket};
        use socket2::Socket;
        let raw = socket.as_raw_socket();
        let s2 = unsafe { Socket::from_raw_socket(raw) };
        let result = s2.set_send_buffer_size(size);
        std::mem::forget(s2);
        result
    }
    #[cfg(unix)]
    {
        use std::os::unix::io::{AsRawFd, FromRawFd};
        use socket2::Socket;
        let raw = socket.as_raw_fd();
        let s2 = unsafe { Socket::from_raw_fd(raw) };
        let result = s2.set_send_buffer_size(size);
        std::mem::forget(s2);
        result
    }
}

#[derive(Debug)]
pub enum SendError {
    Io(std::io::Error),
    Crypto,
    Cancelled,
    AuthFailed,
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SendError::Io(e) => write!(f, "I/O error: {}", e),
            SendError::Crypto => write!(f, "encryption error"),
            SendError::Cancelled => write!(f, "transfer cancelled"),
            SendError::AuthFailed => write!(f, "UDP relay authentication failed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spin_sleep_accuracy() {
        let target = Duration::from_micros(100);
        let start = Instant::now();
        spin_sleep(target);
        let elapsed = start.elapsed();
        assert!(elapsed >= target);
        assert!(elapsed < target + Duration::from_micros(200));
    }
}
