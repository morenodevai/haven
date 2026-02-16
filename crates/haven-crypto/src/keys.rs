use aes_gcm::aead::OsRng;
use aes_gcm::aead::rand_core::RngCore;
use anyhow::Result;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};

/// Generate a random 256-bit key for AES-256-GCM.
/// In Phase 0, this key is shared between all channel members.
pub fn generate_channel_key() -> [u8; 32] {
    let mut key = [0u8; 32];
    OsRng.fill_bytes(&mut key);
    key
}

/// Encode a key to base64 for display/sharing.
pub fn key_to_base64(key: &[u8; 32]) -> String {
    BASE64.encode(key)
}

/// Decode a base64 key.
pub fn key_from_base64(encoded: &str) -> Result<[u8; 32]> {
    let bytes = BASE64.decode(encoded)?;
    let key: [u8; 32] = bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("Invalid key length"))?;
    Ok(key)
}
