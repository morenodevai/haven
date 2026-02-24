use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use aes_gcm::aead::Aead;
use sha2::{Sha256, Digest};

/// Derive an encryption key from a master key and salt using SHA-256.
/// This matches the client-side key derivation.
pub fn derive_key(master_key: &[u8], salt: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(master_key);
    hasher.update(salt);
    let result = hasher.finalize();
    let mut key = [0u8; 32];
    key.copy_from_slice(&result);
    key
}

/// Derive a deterministic 12-byte nonce for a given chunk index.
///
/// Computed as SHA-256(key || chunk_index_le)[..12], which is unique per
/// (key, chunk_index) pair. Since each transfer uses a distinct key this
/// guarantees no nonce reuse across or within transfers.
pub fn derive_chunk_nonce(key: &[u8; 32], chunk_index: u64) -> [u8; 12] {
    let mut hasher = Sha256::new();
    hasher.update(key);
    hasher.update(chunk_index.to_le_bytes());
    let digest = hasher.finalize();
    let mut nonce = [0u8; 12];
    nonce.copy_from_slice(&digest[..12]);
    nonce
}

/// Encrypt a chunk with AES-256-GCM using a caller-supplied nonce.
/// Returns nonce (12 bytes) + ciphertext (with 16-byte auth tag appended by aes-gcm).
pub fn encrypt_chunk_with_nonce(key: &[u8; 32], plaintext: &[u8], nonce_bytes: [u8; 12]) -> Result<Vec<u8>, String> {
    let cipher = Aes256Gcm::new(key.into());
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| format!("Encryption failed: {}", e))?;

    // Output: [nonce(12)][ciphertext+tag]
    let mut output = Vec::with_capacity(12 + ciphertext.len());
    output.extend_from_slice(&nonce_bytes);
    output.extend_from_slice(&ciphertext);
    Ok(output)
}

/// Decrypt a chunk with AES-256-GCM.
/// Input format: [nonce(12)][ciphertext+tag].
pub fn decrypt_chunk(key: &[u8; 32], data: &[u8]) -> Result<Vec<u8>, String> {
    if data.len() < 12 {
        return Err("Data too short for nonce".into());
    }

    let cipher = Aes256Gcm::new(key.into());
    let nonce = Nonce::from_slice(&data[..12]);
    let ciphertext = &data[12..];

    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| format!("Decryption failed: {}", e))
}
