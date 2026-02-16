use aes_gcm::{
    Aes256Gcm, Key, Nonce,
    aead::{Aead, KeyInit, OsRng, rand_core::RngCore},
};
use anyhow::{Result, anyhow};

/// Encrypt a plaintext message with AES-256-GCM.
/// Returns (ciphertext, nonce).
pub fn encrypt_message(key: &[u8; 32], plaintext: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));

    let mut nonce_bytes = [0u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| anyhow!("Encryption failed: {}", e))?;

    Ok((ciphertext, nonce_bytes.to_vec()))
}

/// Decrypt a ciphertext message with AES-256-GCM.
pub fn decrypt_message(key: &[u8; 32], ciphertext: &[u8], nonce: &[u8]) -> Result<Vec<u8>> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));

    let nonce = Nonce::from_slice(nonce);

    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| anyhow!("Decryption failed: {}", e))?;

    Ok(plaintext)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::generate_channel_key;

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let key = generate_channel_key();
        let message = b"Hello from Haven!";

        let (ciphertext, nonce) = encrypt_message(&key, message).unwrap();
        assert_ne!(&ciphertext, message);

        let decrypted = decrypt_message(&key, &ciphertext, &nonce).unwrap();
        assert_eq!(decrypted, message);
    }

    #[test]
    fn wrong_key_fails() {
        let key1 = generate_channel_key();
        let key2 = generate_channel_key();
        let message = b"Secret message";

        let (ciphertext, nonce) = encrypt_message(&key1, message).unwrap();
        let result = decrypt_message(&key2, &ciphertext, &nonce);
        assert!(result.is_err());
    }
}
