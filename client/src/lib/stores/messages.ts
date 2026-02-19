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
  videoUrl?: string;
  videoName?: string;
}

/** After decryption, detect image/video envelopes and extract fields. */
function parseDecryptedContent(plaintext: string): {
  content: string;
  imageData?: string;
  imageName?: string;
  videoFileId?: string;
  videoMime?: string;
  videoName?: string;
  videoNonce?: string;
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
    if (parsed && parsed.type === "video" && typeof parsed.file_id === "string") {
      return {
        content: parsed.name || "video",
        videoFileId: parsed.file_id,
        videoMime: parsed.mime || "video/mp4",
        videoName: parsed.name,
        videoNonce: parsed.nonce,
      };
    }
  } catch {
    // Not JSON — treat as plain text
  }
  return { content: plaintext };
}

/** Fetch, decrypt, and create a blob URL for a video file. */
async function resolveVideoUrl(
  fileId: string,
  nonce: string,
  mime: string
): Promise<string | undefined> {
  try {
    const key = get(channelKey);
    if (!key) return undefined;

    const encryptedBytes = await api.downloadFile(fileId);
    // Convert encrypted bytes to base64 for the decrypt command (FileReader handles any size)
    const encryptedBlob = new Blob([new Uint8Array(encryptedBytes)]);
    const ciphertextB64 = await new Promise<string>((resolve, reject) => {
      const reader = new FileReader();
      reader.onload = () => {
        const dataUrl = reader.result as string;
        resolve(dataUrl.split(",")[1]);
      };
      reader.onerror = reject;
      reader.readAsDataURL(encryptedBlob);
    });

    // Decrypt — result is the original base64-encoded video data
    const videoBase64 = await crypto.decrypt(key, ciphertextB64, nonce);

    // Decode base64 to raw bytes using fetch (handles any size)
    const rawResponse = await fetch(`data:${mime};base64,${videoBase64}`);
    const videoBytes = new Uint8Array(await rawResponse.arrayBuffer());

    const blob = new Blob([videoBytes], { type: mime });
    return URL.createObjectURL(blob);
  } catch (e) {
    console.error("Failed to resolve video:", e);
    return undefined;
  }
}

export const messages = writable<DecryptedMessage[]>([]);
export const channelKey = writable<string | null>(null);

// Default channel ID (seeded in migrations)
export const GENERAL_CHANNEL_ID = "00000000-0000-0000-0000-000000000001";

// Default shared key — all users use the same key for MVP
// 32 bytes (AES-256) base64-encoded
export const DEFAULT_CHANNEL_KEY = "QUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUE=";

// SECURITY NOTE: The AES channel key is currently stored in localStorage, which
// is accessible to any JavaScript running in the page context (XSS risk).
// The CSP headers mitigate this, but do not fully eliminate the threat.
//
// Migration path (TODO):
//   1. Wire up the Tauri-side KeyStore (haven-crypto) for secure native key storage, or
//   2. Use tauri-plugin-store with encryption for on-disk key persistence.
//
// The key should also be cleared when the user logs out or switches channels.

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
        const plaintext = await crypto.decrypt(key, msg.ciphertext, msg.nonce);
        const parsed = parseDecryptedContent(plaintext);

        let videoUrl: string | undefined;
        let videoName: string | undefined;

        if (parsed.videoFileId) {
          videoUrl = await resolveVideoUrl(
            parsed.videoFileId,
            parsed.videoNonce ?? "",
            parsed.videoMime ?? "video/mp4"
          );
          videoName = parsed.videoName;
        }

        decrypted.push({
          id: msg.id,
          channelId: msg.channel_id,
          authorId: msg.author_id,
          authorUsername: msg.author_username,
          content: parsed.content,
          imageData: parsed.imageData,
          imageName: parsed.imageName,
          videoUrl,
          videoName,
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
    const plaintext = await crypto.decrypt(key, data.ciphertext, data.nonce);
    const parsed = parseDecryptedContent(plaintext);

    let videoUrl: string | undefined;
    let videoName: string | undefined;

    if (parsed.videoFileId) {
      videoUrl = await resolveVideoUrl(
        parsed.videoFileId,
        parsed.videoNonce ?? "",
        parsed.videoMime ?? "video/mp4"
      );
      videoName = parsed.videoName;
    }

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
        videoUrl,
        videoName,
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
      const hasGroup = msg.reactions.some((r) => r.emoji === data.emoji);
      let reactions: ReactionGroup[];
      if (hasGroup) {
        reactions = msg.reactions.map((r) => {
          if (r.emoji !== data.emoji) return r;
          if (r.user_ids.includes(data.user_id)) return r;
          const user_ids = [...r.user_ids, data.user_id];
          return { emoji: r.emoji, count: user_ids.length, user_ids };
        });
      } else {
        reactions = [
          ...msg.reactions,
          { emoji: data.emoji, count: 1, user_ids: [data.user_id] },
        ];
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

