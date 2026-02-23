/// Integration test: send a file to localhost and verify it arrives intact.
///
/// This test creates a temp file, sends it over UDP loopback, receives it,
/// and verifies the output matches byte-for-byte.

use std::fs;
use std::io::Write;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use haven_transfer::congestion::CongestionConfig;
use haven_transfer::crypto::TransferCrypto;
use haven_transfer::protocol::MAX_PLAINTEXT_PER_PACKET;
use haven_transfer::receiver::{self, ReceiverEvent, RecvConfig};
use haven_transfer::sender::{self, SendConfig, SenderCommand};
use haven_transfer::stats::TransferStats;

#[test]
fn loopback_transfer_small_file() {
    loopback_transfer(1024 * 10); // 10 KB
}

#[test]
fn loopback_transfer_medium_file() {
    loopback_transfer(1024 * 1024); // 1 MB
}

#[test]
fn loopback_transfer_exact_chunk_boundary() {
    loopback_transfer(MAX_PLAINTEXT_PER_PACKET * 3); // exactly 3 chunks
}

fn loopback_transfer(file_size: usize) {
    let _ = env_logger::try_init();

    let dir = std::env::temp_dir().join(format!("haven_transfer_test_{}", file_size));
    let _ = fs::create_dir_all(&dir);

    let input_path = dir.join("input.bin");
    let output_path = dir.join("output.bin");

    // Create test file with known pattern
    {
        let mut f = fs::File::create(&input_path).unwrap();
        let mut data = vec![0u8; file_size];
        for (i, byte) in data.iter_mut().enumerate() {
            *byte = (i % 251) as u8; // prime modulus for good distribution
        }
        f.write_all(&data).unwrap();
    }

    let total_chunks =
        (file_size as u64 + MAX_PLAINTEXT_PER_PACKET as u64 - 1) / MAX_PLAINTEXT_PER_PACKET as u64;

    // Shared key and salt
    let mut master_key = [0u8; 32];
    master_key[0] = 0xDE;
    master_key[31] = 0xAD;
    let salt = TransferCrypto::random_salt();
    let session_id: u32 = (file_size % 100_000) as u32 + 1;

    // Bind receiver first to get its address
    let recv_socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let recv_addr: SocketAddr = recv_socket.local_addr().unwrap();
    drop(recv_socket); // release so the receiver can bind

    let recv_stats = Arc::new(TransferStats::new());
    let recv_cancelled = Arc::new(AtomicBool::new(false));
    let (recv_event_tx, recv_event_rx) = crossbeam_channel::bounded::<ReceiverEvent>(256);

    let recv_config = RecvConfig {
        session_id,
        output_path: output_path.to_str().unwrap().to_string(),
        file_size: file_size as u64,
        total_chunks,
        listen_addr: recv_addr,
        master_key,
        salt,
        jwt_token: String::new(),  // empty = skip auth (loopback)
        relay_addr: None,
    };

    let rs = recv_stats.clone();
    let rc = recv_cancelled.clone();

    // Start receiver thread
    let recv_handle = thread::spawn(move || {
        receiver::run_receiver(recv_config, recv_event_tx, rs, rc)
    });

    // Small delay to let receiver bind
    thread::sleep(Duration::from_millis(50));

    // Start sender
    let send_stats = Arc::new(TransferStats::new());
    let send_cancelled = Arc::new(AtomicBool::new(false));
    let (send_cmd_tx, send_cmd_rx) = crossbeam_channel::bounded::<SenderCommand>(256);

    let send_config = SendConfig {
        session_id,
        file_path: input_path.to_str().unwrap().to_string(),
        dest_addr: recv_addr,
        master_key,
        salt,
        congestion: CongestionConfig {
            initial_rate: 100_000_000, // 100 MB/s (fast for loopback)
            max_rate: 1_000_000_000,
            update_interval: Duration::from_millis(10),
            ..Default::default()
        },
        jwt_token: String::new(),  // empty = skip auth (loopback)
    };

    let ss = send_stats.clone();
    let sc = send_cancelled.clone();

    let send_handle = thread::spawn(move || {
        sender::run_sender(send_config, send_cmd_rx, ss, sc)
    });

    // Bridge NACK events from receiver back to sender
    let bridge_cancelled = recv_cancelled.clone();
    let bridge_handle = thread::spawn(move || {
        while !bridge_cancelled.load(Ordering::Relaxed) {
            match recv_event_rx.recv_timeout(Duration::from_millis(100)) {
                Ok(ReceiverEvent::Nack(missing)) => {
                    let _ = send_cmd_tx.send(SenderCommand::Nack(missing));
                }
                Ok(ReceiverEvent::Done { total_bytes }) => {
                    // Tell sender we're done
                    let ack_seq = (total_bytes + MAX_PLAINTEXT_PER_PACKET as u64 - 1)
                        / MAX_PLAINTEXT_PER_PACKET as u64;
                    let _ = send_cmd_tx.send(SenderCommand::AckUpTo(ack_seq));
                    break;
                }
                Ok(ReceiverEvent::Progress { .. }) => {}
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
            }
        }
    });

    // Wait for completion with timeout
    let send_result = send_handle.join().expect("sender panicked");
    let recv_result = recv_handle.join().expect("receiver panicked");
    bridge_handle.join().expect("bridge panicked");

    // Check results
    let sr = send_result.expect("sender failed");
    let rr = recv_result.expect("receiver failed");

    assert_eq!(sr.total_bytes, file_size as u64);
    assert_eq!(rr.total_bytes, file_size as u64);

    // Compare files byte-by-byte
    let input_data = fs::read(&input_path).unwrap();
    let output_data = fs::read(&output_path).unwrap();
    assert_eq!(input_data.len(), output_data.len(), "file sizes differ");
    assert_eq!(input_data, output_data, "file contents differ");

    println!(
        "Transfer of {} bytes complete in {:.2}s — {} B/s, {} retransmits",
        sr.total_bytes,
        sr.elapsed.as_secs_f64(),
        sr.throughput_bps,
        sr.retransmits
    );

    // Cleanup
    let _ = fs::remove_file(&input_path);
    let _ = fs::remove_file(&output_path);
}
