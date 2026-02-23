/// Transfer statistics — shared between the transfer threads and the FFI caller.
/// All fields are atomic for lock-free reads from the UI thread.

use std::sync::atomic::{AtomicU64, Ordering};

pub struct TransferStats {
    /// Total file size in bytes.
    pub total_bytes: AtomicU64,
    /// Total number of chunks.
    pub total_chunks: AtomicU64,
    /// Bytes transferred so far.
    pub bytes_transferred: AtomicU64,
    /// Chunks transferred so far.
    pub chunks_transferred: AtomicU64,
    /// Number of retransmitted packets.
    pub retransmits: AtomicU64,
    /// Current sending rate in bytes/sec.
    pub current_rate: AtomicU64,
}

impl TransferStats {
    pub fn new() -> Self {
        TransferStats {
            total_bytes: AtomicU64::new(0),
            total_chunks: AtomicU64::new(0),
            bytes_transferred: AtomicU64::new(0),
            chunks_transferred: AtomicU64::new(0),
            retransmits: AtomicU64::new(0),
            current_rate: AtomicU64::new(0),
        }
    }

    pub fn set_total(&self, bytes: u64, chunks: u64) {
        self.total_bytes.store(bytes, Ordering::Relaxed);
        self.total_chunks.store(chunks, Ordering::Relaxed);
    }

    pub fn update(&self, bytes: u64, chunks: u64, retransmits: u64, rate: u64) {
        self.bytes_transferred.store(bytes, Ordering::Relaxed);
        self.chunks_transferred.store(chunks, Ordering::Relaxed);
        self.retransmits.store(retransmits, Ordering::Relaxed);
        self.current_rate.store(rate, Ordering::Relaxed);
    }

    /// Progress as a fraction 0.0 - 1.0.
    pub fn progress(&self) -> f64 {
        let total = self.total_bytes.load(Ordering::Relaxed);
        if total == 0 {
            return 0.0;
        }
        let done = self.bytes_transferred.load(Ordering::Relaxed);
        (done as f64 / total as f64).min(1.0)
    }
}
