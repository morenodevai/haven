import { writable, get } from "svelte/store";
import * as api from "../ipc/api";
import * as crypto from "../ipc/crypto";

export interface DecryptedMessage {
  id: string;
  channelId: string;
  authorId: string;
  authorUsername: string;
  content: string;
  timestamp: string;
}

export const messages = writable<DecryptedMessage[]>([]);
export const channelKey = writable<string | null>(null);

// Default channel ID (seeded in migrations)
export const GENERAL_CHANNEL_ID = "00000000-0000-0000-0000-000000000001";

// Restore key from localStorage
const savedKey = localStorage.getItem("haven_channel_key");
if (savedKey) {
  channelKey.set(savedKey);
}

export function setChannelKey(key: string) {
  localStorage.setItem("haven_channel_key", key);
  channelKey.set(key);
}

export async function loadMessages() {
  const key = get(channelKey);
  if (!key) return;

  try {
    const raw = await api.getMessages(GENERAL_CHANNEL_ID);
    const decrypted: DecryptedMessage[] = [];

    for (const msg of raw) {
      try {
        // Convert byte array to base64
        const ciphertextB64 = bytesToBase64(msg.ciphertext);
        const nonceB64 = bytesToBase64(msg.nonce);

        const content = await crypto.decrypt(key, ciphertextB64, nonceB64);
        decrypted.push({
          id: msg.id,
          channelId: msg.channel_id,
          authorId: msg.author_id,
          authorUsername: msg.author_username,
          content,
          timestamp: msg.created_at,
        });
      } catch {
        decrypted.push({
          id: msg.id,
          channelId: msg.channel_id,
          authorId: msg.author_id,
          authorUsername: msg.author_username,
          content: "[Unable to decrypt]",
          timestamp: msg.created_at,
        });
      }
    }

    // Messages come in DESC order from API, reverse for display
    messages.set(decrypted.reverse());
  } catch (e) {
    console.error("Failed to load messages:", e);
  }
}

export async function sendMessage(content: string) {
  const key = get(channelKey);
  if (!key) throw new Error("No channel key set");

  const encrypted = await crypto.encrypt(key, content);
  await api.sendMessage(GENERAL_CHANNEL_ID, encrypted.ciphertext, encrypted.nonce);
}

export async function handleIncomingMessage(event: any) {
  const key = get(channelKey);
  if (!key) return;

  const data = event.data;
  try {
    const ciphertextB64 = bytesToBase64(data.ciphertext);
    const nonceB64 = bytesToBase64(data.nonce);
    const content = await crypto.decrypt(key, ciphertextB64, nonceB64);

    messages.update((msgs) => [
      ...msgs,
      {
        id: data.id,
        channelId: data.channel_id,
        authorId: data.author_id,
        authorUsername: data.author_username,
        content,
        timestamp: data.timestamp,
      },
    ]);
  } catch {
    messages.update((msgs) => [
      ...msgs,
      {
        id: data.id,
        channelId: data.channel_id,
        authorId: data.author_id,
        authorUsername: data.author_username,
        content: "[Unable to decrypt]",
        timestamp: data.timestamp,
      },
    ]);
  }
}

function bytesToBase64(data: number[] | string): string {
  if (typeof data === "string") return data;
  return btoa(String.fromCharCode(...data));
}
