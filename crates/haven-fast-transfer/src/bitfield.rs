/// Per-chunk frame tracking using a compact bitfield.
///
/// Each chunk can have up to MAX_FRAMES_PER_CHUNK frames (2997 for 4MB chunks).
/// We use `[u64; 47]` = 3008 bits, enough to track all frames.

use crate::protocol::MAX_FRAMES_PER_CHUNK;

/// Number of u64 words needed: ceil(MAX_FRAMES_PER_CHUNK / 64).
const BITFIELD_WORDS: usize = (MAX_FRAMES_PER_CHUNK + 63) / 64;

/// Compact bitfield tracking which frames have been received for a chunk.
#[derive(Clone)]
pub struct ChunkBitfield {
    bits: [u64; BITFIELD_WORDS],
    frame_count: u16,
    received_count: u16,
}

impl ChunkBitfield {
    /// Create a new bitfield for a chunk with `frame_count` frames.
    pub fn new(frame_count: u16) -> Self {
        Self {
            bits: [0u64; BITFIELD_WORDS],
            frame_count,
            received_count: 0,
        }
    }

    /// Mark a frame as received. Returns true if it was newly received (not duplicate).
    #[inline]
    pub fn set(&mut self, frame_index: u16) -> bool {
        let idx = frame_index as usize;
        let word = idx / 64;
        let bit = idx % 64;
        if word >= BITFIELD_WORDS {
            return false;
        }
        let mask = 1u64 << bit;
        if self.bits[word] & mask != 0 {
            return false; // already set
        }
        self.bits[word] |= mask;
        self.received_count += 1;
        true
    }

    /// Check if a frame has been received.
    #[inline]
    pub fn get(&self, frame_index: u16) -> bool {
        let idx = frame_index as usize;
        let word = idx / 64;
        let bit = idx % 64;
        if word >= BITFIELD_WORDS {
            return false;
        }
        self.bits[word] & (1u64 << bit) != 0
    }

    /// Returns true if all frames have been received.
    #[inline]
    pub fn is_complete(&self) -> bool {
        self.received_count >= self.frame_count
    }

    /// Number of frames received.
    #[inline]
    pub fn received(&self) -> u16 {
        self.received_count
    }

    /// Total frame count.
    #[inline]
    pub fn total(&self) -> u16 {
        self.frame_count
    }

    /// Collect indices of all missing frames.
    pub fn missing_frames(&self) -> Vec<u16> {
        let mut missing = Vec::new();
        for i in 0..self.frame_count {
            if !self.get(i) {
                missing.push(i);
            }
        }
        missing
    }

    /// Number of missing frames.
    #[inline]
    pub fn missing_count(&self) -> u16 {
        self.frame_count - self.received_count
    }

    /// Reset the bitfield for reuse.
    pub fn reset(&mut self, frame_count: u16) {
        self.bits = [0u64; BITFIELD_WORDS];
        self.frame_count = frame_count;
        self.received_count = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_operations() {
        let mut bf = ChunkBitfield::new(100);
        assert!(!bf.is_complete());
        assert_eq!(bf.missing_count(), 100);

        assert!(bf.set(0));
        assert!(!bf.set(0)); // duplicate
        assert_eq!(bf.received(), 1);
        assert!(bf.get(0));
        assert!(!bf.get(1));

        for i in 1..100 {
            bf.set(i);
        }
        assert!(bf.is_complete());
        assert_eq!(bf.missing_frames().len(), 0);
    }

    #[test]
    fn test_missing_frames() {
        let mut bf = ChunkBitfield::new(10);
        bf.set(0);
        bf.set(2);
        bf.set(5);
        bf.set(9);
        let missing = bf.missing_frames();
        assert_eq!(missing, vec![1, 3, 4, 6, 7, 8]);
    }

    #[test]
    fn test_max_frames() {
        let mut bf = ChunkBitfield::new(MAX_FRAMES_PER_CHUNK as u16);
        for i in 0..MAX_FRAMES_PER_CHUNK as u16 {
            assert!(bf.set(i));
        }
        assert!(bf.is_complete());
    }
}
