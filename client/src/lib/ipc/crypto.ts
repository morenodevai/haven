import { invoke } from "@tauri-apps/api/core";

export interface EncryptedPayload {
  ciphertext: string;
  nonce: string;
}

export async function generateKey(): Promise<string> {
  return await invoke<string>("generate_key");
}

export async function encrypt(
  keyB64: string,
  plaintext: string
): Promise<EncryptedPayload> {
  return await invoke<EncryptedPayload>("encrypt", {
    keyB64,
    plaintext,
  });
}

export async function decrypt(
  keyB64: string,
  ciphertextB64: string,
  nonceB64: string
): Promise<string> {
  return await invoke<string>("decrypt", {
    keyB64,
    ciphertextB64: ciphertextB64,
    nonceB64: nonceB64,
  });
}
