use haven_crypto::encrypt::{decrypt_message, encrypt_message};
use haven_crypto::keys::{generate_channel_key, key_from_base64, key_to_base64};
use std::sync::Mutex;
use tauri::State;

/// In-memory channel key storage.
/// Phase 0: single shared key for #general.
pub struct KeyStore {
    channel_key: Mutex<Option<[u8; 32]>>,
}

impl Default for KeyStore {
    fn default() -> Self {
        Self {
            channel_key: Mutex::new(None),
        }
    }
}

#[tauri::command]
pub fn generate_key() -> Result<String, String> {
    let key = generate_channel_key();
    Ok(key_to_base64(&key))
}

#[tauri::command]
pub fn encrypt(key_b64: String, plaintext: String) -> Result<EncryptedPayload, String> {
    let key = key_from_base64(&key_b64).map_err(|e| e.to_string())?;
    let (ciphertext, nonce) =
        encrypt_message(&key, plaintext.as_bytes()).map_err(|e| e.to_string())?;
    Ok(EncryptedPayload {
        ciphertext: base64_encode(&ciphertext),
        nonce: base64_encode(&nonce),
    })
}

#[tauri::command]
pub fn decrypt(key_b64: String, ciphertext_b64: String, nonce_b64: String) -> Result<String, String> {
    let key = key_from_base64(&key_b64).map_err(|e| e.to_string())?;
    let ciphertext = base64_decode(&ciphertext_b64).map_err(|e| e.to_string())?;
    let nonce = base64_decode(&nonce_b64).map_err(|e| e.to_string())?;
    let plaintext = decrypt_message(&key, &ciphertext, &nonce).map_err(|e| e.to_string())?;
    String::from_utf8(plaintext).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn export_key(key_state: State<KeyStore>) -> Result<Option<String>, String> {
    let guard = key_state.channel_key.lock().map_err(|e| e.to_string())?;
    Ok(guard.map(|k| key_to_base64(&k)))
}

#[tauri::command]
pub fn import_key(key_b64: String, key_state: State<KeyStore>) -> Result<(), String> {
    let key = key_from_base64(&key_b64).map_err(|e| e.to_string())?;
    let mut guard = key_state.channel_key.lock().map_err(|e| e.to_string())?;
    *guard = Some(key);
    Ok(())
}

#[derive(serde::Serialize)]
pub struct EncryptedPayload {
    pub ciphertext: String,
    pub nonce: String,
}

fn base64_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

fn base64_decode(data: &str) -> Result<Vec<u8>, base64::DecodeError> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(data)
}
