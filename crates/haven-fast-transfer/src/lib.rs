/// Haven Fast Transfer: UDP blast file transfer library.
///
/// Provides high-speed file transfer over UDP with:
/// - 3-thread sender pipeline: reader → encryptor → blaster
/// - 3-thread receiver pipeline: UDP vacuum → assembler → writer
/// - Per-chunk bitfield frame tracking
/// - NACK-based retransmission
/// - Rate control with loss-based backoff
/// - AES-256-GCM encryption with deterministic nonces
/// - SHA-256 integrity verification

pub mod bitfield;
pub mod logging;
pub mod protocol;
pub mod receiver;
pub mod sender;

// Re-export key types for convenience.
pub use bitfield::ChunkBitfield;
pub use logging::{NullLogger, TracingLogger, TransferLogger};
pub use protocol::{
    decode_frame_header, encode_frame, frame_payload, frames_for_chunk, FrameHeader,
    CHUNK_SIZE, ENCRYPTED_CHUNK_SIZE, ENCRYPTION_OVERHEAD, FRAME_HEADER, FRAME_MAX,
    FRAME_PAYLOAD, MAX_FRAMES_PER_CHUNK,
};
pub use receiver::{NackCallback, ReceiverConfig, ReceiverProgress, run_receiver};
pub use sender::{
    ChunkAckMessage, NackMessage, RawSenderConfig, SendResult, SenderConfig, SenderProgress,
    run_raw_sender, run_sender,
};
