import { writable, get } from "svelte/store";
import * as api from "../ipc/api";
import type { ReactionGroup } from "../ipc/api";
import * as crypto from "../ipc/crypto";

export type { ReactionGroup } from "../ipc/api";

export interface DecryptedMessage {
  id: string;
  channelId: string;
  authorId: string;
  authorUsername: string;
  content: string;
  timestamp: string;
  reactions: ReactionGroup[];
  imageData?: string;
  imageName?: string;
}

/** After decryption, detect image envelopes and extract fields. */
function parseDecryptedContent(plaintext: string): {
  content: string;
  imageData?: string;
  imageName?: string;
} {
  try {
    const parsed = JSON.parse(plaintext);
    if (parsed && parsed.type === "image" && typeof parsed.data === "string") {
      const mime = parsed.mime || "image/jpeg";
      return {
        content: parsed.name || "image",
        imageData: `data:${mime};base64,${parsed.data}`,
        imageName: parsed.name,
      };
    }
  } catch {
    // Not JSON — treat as plain text
  }
  return { content: plaintext };
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

        const plaintext = await crypto.decrypt(key, ciphertextB64, nonceB64);
        const parsed = parseDecryptedContent(plaintext);
        decrypted.push({
          id: msg.id,
          channelId: msg.channel_id,
          authorId: msg.author_id,
          authorUsername: msg.author_username,
          content: parsed.content,
          imageData: parsed.imageData,
          imageName: parsed.imageName,
          timestamp: msg.created_at,
          reactions: msg.reactions ?? [],
        });
      } catch {
        decrypted.push({
          id: msg.id,
          channelId: msg.channel_id,
          authorId: msg.author_id,
          authorUsername: msg.author_username,
          content: "[Unable to decrypt]",
          timestamp: msg.created_at,
          reactions: msg.reactions ?? [],
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
    const plaintext = await crypto.decrypt(key, ciphertextB64, nonceB64);
    const parsed = parseDecryptedContent(plaintext);

    messages.update((msgs) => [
      ...msgs,
      {
        id: data.id,
        channelId: data.channel_id,
        authorId: data.author_id,
        authorUsername: data.author_username,
        content: parsed.content,
        imageData: parsed.imageData,
        imageName: parsed.imageName,
        timestamp: data.timestamp,
        reactions: [],
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
        reactions: [],
      },
    ]);
  }
}

// -- Reaction handlers --

export function handleReactionAdd(event: any) {
  const data = event.data;
  messages.update((msgs) =>
    msgs.map((msg) => {
      if (msg.id !== data.message_id) return msg;
      const reactions = [...msg.reactions];
      const existing = reactions.find((r) => r.emoji === data.emoji);
      if (existing) {
        if (!existing.user_ids.includes(data.user_id)) {
          existing.user_ids = [...existing.user_ids, data.user_id];
          existing.count = existing.user_ids.length;
        }
      } else {
        reactions.push({
          emoji: data.emoji,
          count: 1,
          user_ids: [data.user_id],
        });
      }
      return { ...msg, reactions };
    })
  );
}

export function handleReactionRemove(event: any) {
  const data = event.data;
  messages.update((msgs) =>
    msgs.map((msg) => {
      if (msg.id !== data.message_id) return msg;
      let reactions = msg.reactions
        .map((r) => {
          if (r.emoji !== data.emoji) return r;
          const user_ids = r.user_ids.filter((id) => id !== data.user_id);
          return { ...r, user_ids, count: user_ids.length };
        })
        .filter((r) => r.count > 0);
      return { ...msg, reactions };
    })
  );
}

export async function toggleReaction(messageId: string, emoji: string) {
  await api.toggleReaction(GENERAL_CHANNEL_ID, messageId, emoji);
}

function bytesToBase64(data: number[] | string): string {
  if (typeof data === "string") return data;
  return btoa(String.fromCharCode(...data));
}
