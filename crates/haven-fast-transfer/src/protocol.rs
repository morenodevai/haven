/// UDP frame format for fast file transfer.
///
/// ```text
/// [0..16]   Transfer ID (UUID, 16 bytes)
/// [16..20]  Chunk index (u32 BE)
/// [20..22]  Frame index within chunk (u16 BE)
/// [22..24]  Frame count for this chunk (u16 BE)
/// [24..]    Encrypted payload slice (up to 1400 bytes)
/// ```
///
/// 24-byte header + up to 1400 bytes payload = 1424 bytes max.
/// Well within 1472-byte MTU limit (1500 - 20 IP - 8 UDP).

/// Maximum payload bytes per UDP frame.
pub const FRAME_PAYLOAD: usize = 1400;

/// Header size in bytes.
pub const FRAME_HEADER: usize = 24;

/// Maximum UDP frame size (header + payload).
pub const FRAME_MAX: usize = FRAME_HEADER + FRAME_PAYLOAD;

/// Chunk size: 4 MB plaintext. Encrypted = plaintext + 28 (12 nonce + 16 tag).
pub const CHUNK_SIZE: usize = 4 * 1024 * 1024;

/// Encrypted chunk overhead: 12-byte nonce + 16-byte GCM tag.
pub const ENCRYPTION_OVERHEAD: usize = 28;

/// Maximum encrypted chunk size.
pub const ENCRYPTED_CHUNK_SIZE: usize = CHUNK_SIZE + ENCRYPTION_OVERHEAD;

/// Maximum frames per chunk: ceil(4_194_332 / 1400) = 2997.
pub const MAX_FRAMES_PER_CHUNK: usize = (ENCRYPTED_CHUNK_SIZE + FRAME_PAYLOAD - 1) / FRAME_PAYLOAD;

/// Number of encrypted chunks to cache in sender for retransmit.
pub const SENDER_CACHE_SIZE: usize = 8;

/// Receiver ring buffer size in frames.
pub const RING_BUFFER_FRAMES: usize = 16384;

/// OS receive buffer size (32 MB).
pub const UDP_RECV_BUFFER: usize = 32 * 1024 * 1024;

/// NACK scan interval in milliseconds.
pub const NACK_SCAN_INTERVAL_MS: u64 = 50;

/// Initial send rate in bytes per second (800 Mbps).
pub const INITIAL_RATE_BPS: u64 = 800_000_000 / 8;

/// Rate decrease factor when loss > 10%.
pub const RATE_DECREASE: f64 = 0.80;

/// Rate increase factor when loss < 1%.
pub const RATE_INCREASE: f64 = 1.10;

/// Loss threshold to trigger rate decrease.
pub const LOSS_THRESHOLD_HIGH: f64 = 0.10;

/// Loss threshold below which we increase rate.
pub const LOSS_THRESHOLD_LOW: f64 = 0.01;

/// Encode a UDP frame into the provided buffer. Returns bytes written.
///
/// # Panics
/// Panics if `buf` is smaller than `FRAME_HEADER + payload.len()`.
pub fn encode_frame(
    buf: &mut [u8],
    transfer_id: &[u8; 16],
    chunk_index: u32,
    frame_index: u16,
    frame_count: u16,
    payload: &[u8],
) -> usize {
    let total = FRAME_HEADER + payload.len();
    assert!(buf.len() >= total);
    assert!(payload.len() <= FRAME_PAYLOAD);

    buf[0..16].copy_from_slice(transfer_id);
    buf[16..20].copy_from_slice(&chunk_index.to_be_bytes());
    buf[20..22].copy_from_slice(&frame_index.to_be_bytes());
    buf[22..24].copy_from_slice(&frame_count.to_be_bytes());
    buf[FRAME_HEADER..total].copy_from_slice(payload);
    total
}

/// Parsed UDP frame header.
#[derive(Debug, Clone, Copy)]
pub struct FrameHeader {
    pub transfer_id: [u8; 16],
    pub chunk_index: u32,
    pub frame_index: u16,
    pub frame_count: u16,
}

/// Decode a frame header from raw bytes. Returns None if too short.
pub fn decode_frame_header(data: &[u8]) -> Option<FrameHeader> {
    if data.len() < FRAME_HEADER {
        return None;
    }
    let mut transfer_id = [0u8; 16];
    transfer_id.copy_from_slice(&data[0..16]);
    let chunk_index = u32::from_be_bytes(data[16..20].try_into().unwrap());
    let frame_index = u16::from_be_bytes(data[20..22].try_into().unwrap());
    let frame_count = u16::from_be_bytes(data[22..24].try_into().unwrap());

    Some(FrameHeader {
        transfer_id,
        chunk_index,
        frame_index,
        frame_count,
    })
}

/// Get the payload slice from a raw frame.
pub fn frame_payload(data: &[u8]) -> &[u8] {
    &data[FRAME_HEADER..]
}

/// Calculate number of frames needed for a chunk of given encrypted size.
pub fn frames_for_chunk(encrypted_size: usize) -> u16 {
    ((encrypted_size + FRAME_PAYLOAD - 1) / FRAME_PAYLOAD) as u16
}
