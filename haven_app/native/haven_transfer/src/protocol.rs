/// Haven Transfer Protocol — packet format and serialization.
///
/// UDP data packets carry encrypted file chunks.
/// Control messages travel over the existing WebSocket on port 3210.
///
/// Packet layout (big-endian):
///   [0..2]   Magic "HT" (0x4854)
///   [2..6]   Session ID (u32)
///   [6..14]  Sequence number (u64)
///   [14..16] Flags (u16)
///   [16..]   Encrypted payload (AES-256-GCM: 12-byte IV + ciphertext + 16-byte tag)
///
/// Max UDP payload = 1472 bytes (1500 MTU - 20 IP - 8 UDP)
/// Header = 16 bytes
/// Encryption overhead = 12 (IV) + 16 (tag) = 28 bytes
/// Max plaintext per packet = 1472 - 16 - 28 = 1428 bytes

pub const MAGIC: [u8; 2] = [0x48, 0x54]; // "HT"
pub const HEADER_SIZE: usize = 16;
pub const MAX_UDP_PAYLOAD: usize = 1472;
pub const ENCRYPTION_OVERHEAD: usize = 28; // 12 IV + 16 GCM tag
pub const MAX_PLAINTEXT_PER_PACKET: usize = MAX_UDP_PAYLOAD - HEADER_SIZE - ENCRYPTION_OVERHEAD;

/// Packet flags.
pub const FLAG_START: u16 = 1 << 0;
pub const FLAG_END: u16 = 1 << 1;
pub const FLAG_RETRANSMIT: u16 = 1 << 2;

/// A data packet header (parsed from wire format).
#[derive(Debug, Clone, Copy)]
pub struct PacketHeader {
    pub session_id: u32,
    pub sequence: u64,
    pub flags: u16,
}

impl PacketHeader {
    /// Serialize header into the first 16 bytes of `buf`.
    pub fn write_to(&self, buf: &mut [u8]) {
        debug_assert!(buf.len() >= HEADER_SIZE);
        buf[0..2].copy_from_slice(&MAGIC);
        buf[2..6].copy_from_slice(&self.session_id.to_be_bytes());
        buf[6..14].copy_from_slice(&self.sequence.to_be_bytes());
        buf[14..16].copy_from_slice(&self.flags.to_be_bytes());
    }

    /// Parse header from buffer. Returns None if magic doesn't match or buffer too short.
    pub fn parse(buf: &[u8]) -> Option<Self> {
        if buf.len() < HEADER_SIZE {
            return None;
        }
        if buf[0..2] != MAGIC {
            return None;
        }
        let session_id = u32::from_be_bytes([buf[2], buf[3], buf[4], buf[5]]);
        let sequence = u64::from_be_bytes([
            buf[6], buf[7], buf[8], buf[9], buf[10], buf[11], buf[12], buf[13],
        ]);
        let flags = u16::from_be_bytes([buf[14], buf[15]]);
        Some(PacketHeader {
            session_id,
            sequence,
            flags,
        })
    }
}

/// Control messages sent over WebSocket (JSON serialized on the Dart/JS side).
/// These are defined here for documentation — the actual serialization happens
/// in the Flutter client and Rust server, not in this crate.
///
/// OFFER:    { type: "transfer_offer", session_id, filename, size, chunk_count, sender_id }
/// ACCEPT:   { type: "transfer_accept", session_id, receiver_id, receiver_addr? }
/// NACK:     { type: "transfer_nack", session_id, missing: [seq1, seq2, ...] }
/// RATE_ADJ: { type: "transfer_rate", session_id, rtt_us, min_rtt_us, loss_count }
/// DONE:     { type: "transfer_done", session_id, total_bytes, sha256 }
/// CANCEL:   { type: "transfer_cancel", session_id, reason }

/// A complete packet ready to send (header + encrypted payload).
pub struct DataPacket {
    pub header: PacketHeader,
    pub encrypted_payload: Vec<u8>, // IV(12) + ciphertext + tag(16)
}

impl DataPacket {
    /// Serialize to wire format.
    pub fn to_bytes(&self) -> Vec<u8> {
        let total = HEADER_SIZE + self.encrypted_payload.len();
        let mut buf = vec![0u8; total];
        self.header.write_to(&mut buf);
        buf[HEADER_SIZE..].copy_from_slice(&self.encrypted_payload);
        buf
    }

    /// Parse from wire format.
    pub fn from_bytes(buf: &[u8]) -> Option<Self> {
        let header = PacketHeader::parse(buf)?;
        if buf.len() <= HEADER_SIZE {
            return None;
        }
        let encrypted_payload = buf[HEADER_SIZE..].to_vec();
        Some(DataPacket {
            header,
            encrypted_payload,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_header() {
        let hdr = PacketHeader {
            session_id: 0xDEADBEEF,
            sequence: 42,
            flags: FLAG_START | FLAG_END,
        };
        let mut buf = [0u8; HEADER_SIZE];
        hdr.write_to(&mut buf);
        let parsed = PacketHeader::parse(&buf).unwrap();
        assert_eq!(parsed.session_id, 0xDEADBEEF);
        assert_eq!(parsed.sequence, 42);
        assert_eq!(parsed.flags, FLAG_START | FLAG_END);
    }

    #[test]
    fn roundtrip_packet() {
        let pkt = DataPacket {
            header: PacketHeader {
                session_id: 1,
                sequence: 100,
                flags: 0,
            },
            encrypted_payload: vec![1, 2, 3, 4, 5],
        };
        let bytes = pkt.to_bytes();
        let parsed = DataPacket::from_bytes(&bytes).unwrap();
        assert_eq!(parsed.header.session_id, 1);
        assert_eq!(parsed.header.sequence, 100);
        assert_eq!(parsed.encrypted_payload, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn reject_bad_magic() {
        let buf = [0xFF, 0xFF, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        assert!(PacketHeader::parse(&buf).is_none());
    }

    #[test]
    fn reject_short_buffer() {
        let buf = [0x48, 0x54, 0, 0];
        assert!(PacketHeader::parse(&buf).is_none());
    }
}
