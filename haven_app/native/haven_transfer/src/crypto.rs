/// AES-256-GCM encryption for transfer packets.
///
/// Key derivation: HKDF-SHA256(master_key, salt=random_32, info="haven-transfer")
///   → 32-byte session key
///
/// Per-packet IV construction (deterministic, no reuse):
///   IV[0..4]  = session_id (big-endian)
///   IV[4..12] = sequence_number (big-endian)
///
/// This guarantees unique IVs across all packets in all sessions — session_id
/// is unique per transfer, sequence is unique within a transfer.
///
/// Wire format per packet payload: IV(12) + ciphertext + GCM_tag(16)

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use hkdf::Hkdf;
use sha2::Sha256;

pub const IV_SIZE: usize = 12;
pub const TAG_SIZE: usize = 16;
pub const KEY_SIZE: usize = 32;

/// Per-transfer encryption context.
pub struct TransferCrypto {
    cipher: Aes256Gcm,
}

impl TransferCrypto {
    /// Derive a session key from the master key and a random salt.
    pub fn new(master_key: &[u8; KEY_SIZE], salt: &[u8; KEY_SIZE]) -> Self {
        let hk = Hkdf::<Sha256>::new(Some(salt), master_key);
        let mut session_key = [0u8; KEY_SIZE];
        hk.expand(b"haven-transfer", &mut session_key)
            .expect("HKDF expand failed — output length is valid");
        let cipher = Aes256Gcm::new_from_slice(&session_key).unwrap();
        // Zero out the derived key from stack
        session_key.fill(0);
        TransferCrypto { cipher }
    }

    /// Create directly from a 32-byte session key (for receiver side where
    /// the session key is transmitted via the control channel).
    pub fn from_session_key(key: &[u8; KEY_SIZE]) -> Self {
        let cipher = Aes256Gcm::new_from_slice(key).unwrap();
        TransferCrypto { cipher }
    }

    /// Build the deterministic 12-byte IV for a given session + sequence.
    fn build_iv(session_id: u32, sequence: u64) -> [u8; IV_SIZE] {
        let mut iv = [0u8; IV_SIZE];
        iv[0..4].copy_from_slice(&session_id.to_be_bytes());
        iv[4..12].copy_from_slice(&sequence.to_be_bytes());
        iv
    }

    /// Encrypt a plaintext chunk. Returns IV + ciphertext + tag.
    pub fn encrypt(
        &self,
        session_id: u32,
        sequence: u64,
        plaintext: &[u8],
    ) -> Result<Vec<u8>, EncryptError> {
        let iv = Self::build_iv(session_id, sequence);
        let nonce = Nonce::from_slice(&iv);

        // AAD = session_id + sequence (binds ciphertext to its position)
        let mut aad = [0u8; 12];
        aad[0..4].copy_from_slice(&session_id.to_be_bytes());
        aad[4..12].copy_from_slice(&sequence.to_be_bytes());

        let ciphertext = self
            .cipher
            .encrypt(nonce, Payload { msg: plaintext, aad: &aad })
            .map_err(|_| EncryptError)?;

        // Wire format: IV(12) + ciphertext_with_tag
        let mut out = Vec::with_capacity(IV_SIZE + ciphertext.len());
        out.extend_from_slice(&iv);
        out.extend_from_slice(&ciphertext);
        Ok(out)
    }

    /// Decrypt a payload (IV + ciphertext + tag). Returns plaintext.
    pub fn decrypt(
        &self,
        session_id: u32,
        sequence: u64,
        encrypted: &[u8],
    ) -> Result<Vec<u8>, DecryptError> {
        if encrypted.len() < IV_SIZE + TAG_SIZE {
            return Err(DecryptError::TooShort);
        }

        let iv = &encrypted[..IV_SIZE];
        let ciphertext_and_tag = &encrypted[IV_SIZE..];
        let nonce = Nonce::from_slice(iv);

        // Verify the IV matches expected session+sequence
        let expected_iv = Self::build_iv(session_id, sequence);
        if iv != expected_iv {
            return Err(DecryptError::IvMismatch);
        }

        let mut aad = [0u8; 12];
        aad[0..4].copy_from_slice(&session_id.to_be_bytes());
        aad[4..12].copy_from_slice(&sequence.to_be_bytes());

        self.cipher
            .decrypt(nonce, Payload { msg: ciphertext_and_tag, aad: &aad })
            .map_err(|_| DecryptError::AuthFailed)
    }

    /// Generate a random 32-byte salt for key derivation.
    pub fn random_salt() -> [u8; KEY_SIZE] {
        let mut salt = [0u8; KEY_SIZE];
        use rand::RngCore;
        rand::thread_rng().fill_bytes(&mut salt);
        salt
    }
}

#[derive(Debug)]
pub struct EncryptError;

#[derive(Debug)]
pub enum DecryptError {
    TooShort,
    IvMismatch,
    AuthFailed,
}

impl std::fmt::Display for EncryptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "encryption failed")
    }
}

impl std::fmt::Display for DecryptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecryptError::TooShort => write!(f, "encrypted payload too short"),
            DecryptError::IvMismatch => write!(f, "IV does not match expected session/sequence"),
            DecryptError::AuthFailed => write!(f, "GCM authentication failed — data corrupted or tampered"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key() -> [u8; KEY_SIZE] {
        let mut key = [0u8; KEY_SIZE];
        key[0] = 0xAA;
        key[31] = 0xBB;
        key
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let master = test_key();
        let salt = TransferCrypto::random_salt();
        let crypto = TransferCrypto::new(&master, &salt);

        let plaintext = b"hello haven transfer protocol";
        let encrypted = crypto.encrypt(1, 42, plaintext).unwrap();

        // Encrypted should be IV(12) + ciphertext(29) + tag(16) = 57 bytes
        assert_eq!(encrypted.len(), IV_SIZE + plaintext.len() + TAG_SIZE);

        let decrypted = crypto.decrypt(1, 42, &encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn wrong_sequence_fails() {
        let master = test_key();
        let salt = TransferCrypto::random_salt();
        let crypto = TransferCrypto::new(&master, &salt);

        let encrypted = crypto.encrypt(1, 42, b"data").unwrap();
        // Try decrypting with wrong sequence — IV mismatch
        assert!(crypto.decrypt(1, 43, &encrypted).is_err());
    }

    #[test]
    fn tampered_data_fails() {
        let master = test_key();
        let salt = TransferCrypto::random_salt();
        let crypto = TransferCrypto::new(&master, &salt);

        let mut encrypted = crypto.encrypt(1, 1, b"sensitive data").unwrap();
        // Flip a byte in the ciphertext
        let mid = IV_SIZE + 5;
        encrypted[mid] ^= 0xFF;
        assert!(crypto.decrypt(1, 1, &encrypted).is_err());
    }

    #[test]
    fn different_sessions_different_ciphertext() {
        let master = test_key();
        let salt1 = TransferCrypto::random_salt();
        let salt2 = TransferCrypto::random_salt();
        let c1 = TransferCrypto::new(&master, &salt1);
        let c2 = TransferCrypto::new(&master, &salt2);

        let e1 = c1.encrypt(1, 1, b"same data").unwrap();
        let e2 = c2.encrypt(1, 1, b"same data").unwrap();
        // Different salts → different session keys → different ciphertext
        assert_ne!(e1, e2);
    }

    #[test]
    fn empty_plaintext() {
        let master = test_key();
        let salt = TransferCrypto::random_salt();
        let crypto = TransferCrypto::new(&master, &salt);

        let encrypted = crypto.encrypt(1, 0, b"").unwrap();
        assert_eq!(encrypted.len(), IV_SIZE + TAG_SIZE); // just IV + tag
        let decrypted = crypto.decrypt(1, 0, &encrypted).unwrap();
        assert!(decrypted.is_empty());
    }
}
