/// Transfer logging trait for structured remote logging.
///
/// Components (sender, file server, receiver) send structured logs
/// to a logging endpoint for real-time debugging.

use std::fmt;

/// Structured log entry for a transfer operation.
#[derive(Debug, Clone)]
pub struct TransferLog {
    pub component: &'static str,
    pub transfer_id: [u8; 16],
    pub event: TransferEvent,
}

/// Transfer events that can be logged.
#[derive(Debug, Clone)]
pub enum TransferEvent {
    /// Sender: chunk encrypted
    ChunkEncrypted {
        chunk_idx: u32,
        size: usize,
        duration_ms: u64,
    },
    /// Sender: frames blasted for a chunk
    FramesBlasted {
        chunk_idx: u32,
        frame_count: u16,
    },
    /// Sender/Receiver: retransmit request
    RetransmitRequest {
        chunk_idx: u32,
        missing_count: u16,
    },
    /// Rate adjustment
    RateAdjusted {
        old_rate_bps: u64,
        new_rate_bps: u64,
        loss_pct: f64,
    },
    /// Receiver: chunk assembled and verified
    ChunkAssembled {
        chunk_idx: u32,
        hash_match: bool,
    },
    /// Receiver: chunk written to disk
    ChunkWritten {
        chunk_idx: u32,
        duration_ms: u64,
    },
    /// NACK sent
    NackSent {
        chunk_idx: u32,
        missing_count: u16,
    },
    /// Blast started (raw sender)
    BlastStarted {
        target: String,
        rate_bps: u64,
        chunk_count: u32,
        file_size: u64,
    },
    /// Blast progress (raw sender, periodic)
    BlastProgress {
        chunks_sent: u32,
        chunks_total: u32,
        rate_bps: u64,
    },
    /// Blast complete (raw sender)
    BlastComplete {
        chunks_sent: u32,
        duration_ms: u64,
        retransmits: u64,
        effective_mbps: f64,
    },
    /// Retransmit sent
    RetransmitSent {
        chunk_idx: u32,
        frame_count: u16,
    },
    /// Transfer complete
    TransferComplete {
        total_bytes: u64,
        duration_ms: u64,
        retransmits: u64,
    },
    /// Error occurred
    Error {
        message: String,
    },
    /// Vacuum thread started
    VacuumStarted {
        bind_addr: String,
    },
    /// Vacuum thread progress
    VacuumProgress {
        frames_received: u64,
        from: String,
    },
    /// Transfer ID mismatch on received frame
    TransferIdMismatch {
        got: [u8; 16],
        from: String,
    },
}

impl fmt::Display for TransferEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ChunkEncrypted { chunk_idx, size, duration_ms } => {
                write!(f, "chunk_encrypted idx={} size={} duration_ms={}", chunk_idx, size, duration_ms)
            }
            Self::FramesBlasted { chunk_idx, frame_count } => {
                write!(f, "frames_blasted idx={} frames={}", chunk_idx, frame_count)
            }
            Self::RetransmitRequest { chunk_idx, missing_count } => {
                write!(f, "retransmit_request idx={} missing={}", chunk_idx, missing_count)
            }
            Self::RateAdjusted { old_rate_bps, new_rate_bps, loss_pct } => {
                write!(f, "rate_adjusted old={} new={} loss={:.1}%", old_rate_bps, new_rate_bps, loss_pct * 100.0)
            }
            Self::ChunkAssembled { chunk_idx, hash_match } => {
                write!(f, "chunk_assembled idx={} hash_match={}", chunk_idx, hash_match)
            }
            Self::ChunkWritten { chunk_idx, duration_ms } => {
                write!(f, "chunk_written idx={} duration_ms={}", chunk_idx, duration_ms)
            }
            Self::NackSent { chunk_idx, missing_count } => {
                write!(f, "nack_sent idx={} missing={}", chunk_idx, missing_count)
            }
            Self::BlastStarted { target, rate_bps, chunk_count, file_size } => {
                write!(f, "blast_started target={} rate_mbps={} chunks={} size={}", target, *rate_bps as f64 * 8.0 / 1_000_000.0, chunk_count, file_size)
            }
            Self::BlastProgress { chunks_sent, chunks_total, rate_bps } => {
                write!(f, "blast_progress chunks={}/{} rate_mbps={:.0}", chunks_sent, chunks_total, *rate_bps as f64 * 8.0 / 1_000_000.0)
            }
            Self::BlastComplete { chunks_sent, duration_ms, retransmits, effective_mbps } => {
                write!(f, "blast_complete chunks={} duration_ms={} retransmits={} effective_mbps={:.1}", chunks_sent, duration_ms, retransmits, effective_mbps)
            }
            Self::RetransmitSent { chunk_idx, frame_count } => {
                write!(f, "retransmit_sent idx={} frames={}", chunk_idx, frame_count)
            }
            Self::TransferComplete { total_bytes, duration_ms, retransmits } => {
                write!(f, "transfer_complete bytes={} duration_ms={} retransmits={}", total_bytes, duration_ms, retransmits)
            }
            Self::Error { message } => {
                write!(f, "error: {}", message)
            }
            Self::VacuumStarted { bind_addr } => {
                write!(f, "vacuum_started bind={}", bind_addr)
            }
            Self::VacuumProgress { frames_received, from } => {
                write!(f, "vacuum_progress frames={} from={}", frames_received, from)
            }
            Self::TransferIdMismatch { got, from } => {
                write!(f, "transfer_id_mismatch got={} from={}", hex::encode(got), from)
            }
        }
    }
}

/// Trait for transfer logging. Implementations can send logs to a WebSocket,
/// write to tracing, or discard them.
pub trait TransferLogger: Send + Sync {
    fn log(&self, entry: TransferLog);
}

/// Logger that uses the `tracing` crate.
pub struct TracingLogger;

impl TransferLogger for TracingLogger {
    fn log(&self, entry: TransferLog) {
        let tid = hex::encode(entry.transfer_id);
        // Use info for key lifecycle events, debug for per-chunk spam
        match &entry.event {
            TransferEvent::VacuumStarted { .. }
            | TransferEvent::VacuumProgress { .. }
            | TransferEvent::TransferIdMismatch { .. }
            | TransferEvent::TransferComplete { .. }
            | TransferEvent::BlastStarted { .. }
            | TransferEvent::BlastProgress { .. }
            | TransferEvent::BlastComplete { .. }
            | TransferEvent::NackSent { .. }
            | TransferEvent::RateAdjusted { .. }
            | TransferEvent::RetransmitSent { .. }
            | TransferEvent::Error { .. } => {
                tracing::info!(
                    component = entry.component,
                    transfer_id = %tid,
                    "{}",
                    entry.event,
                );
            }
            _ => {
                tracing::debug!(
                    component = entry.component,
                    transfer_id = %tid,
                    "{}",
                    entry.event,
                );
            }
        }
    }
}

/// No-op logger that discards all log entries.
pub struct NullLogger;

impl TransferLogger for NullLogger {
    fn log(&self, _entry: TransferLog) {}
}
