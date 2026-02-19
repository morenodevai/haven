import { writable, derived, get } from "svelte/store";
import type { Gateway } from "../ipc/gateway";
import { create, type FileHandle } from "@tauri-apps/plugin-fs";
import { save } from "@tauri-apps/plugin-dialog";
import { channelKey } from "./messages";
import { auth } from "./auth";

// --- Constants ---

const CHUNK_SIZE = 64 * 1024; // 64KB — WebRTC DataChannel safe max
const BUFFER_HIGH = 1 * 1024 * 1024; // 1MB — back-pressure threshold
const BUFFER_LOW = 512 * 1024; // 512KB — resume threshold
const ICE_SERVERS: RTCIceServer[] = [
  { urls: "stun:stun.l.google.com:19302" },
  { urls: "stun:stun1.l.google.com:19302" },
];
const ICE_TIMEOUT = 10_000; // 10s — if P2P fails, fall back to server relay
const RELAY_CHUNK_SIZE = 48 * 1024; // 48KB for relay (smaller due to base64 overhead)

// --- Types ---

export interface Transfer {
  id: string;
  peerId: string;
  peerUsername: string;
  filename: string;
  size: number;
  direction: "send" | "receive";
  status:
    | "pending"
    | "connecting"
    | "transferring"
    | "completed"
    | "failed"
    | "rejected"
    | "cancelled";
  bytesTransferred: number;
  startTime?: number;
}

// Internal state not exposed to UI
interface TransferInternal {
  pc?: RTCPeerConnection;
  dc?: RTCDataChannel;
  cryptoKey?: CryptoKey;
  fileHandle?: FileHandle;
  file?: File;
  relayMode?: boolean; // true = using server relay instead of P2P
  relayChunkIndex?: number; // for receiver: next expected chunk
  iceTimer?: number; // timeout handle for ICE fallback
}

// --- Stores ---

export const transfers = writable<Transfer[]>([]);

export const pendingOffers = derived(transfers, ($t) =>
  $t.filter((t) => t.direction === "receive" && t.status === "pending")
);

export const activeTransfers = derived(transfers, ($t) =>
  $t.filter(
    (t) =>
      t.status === "connecting" ||
      t.status === "transferring" ||
      t.status === "pending"
  )
);

// --- Module state ---

let gw: Gateway | null = null;
const internals = new Map<string, TransferInternal>();

// --- Base64 helpers ---

function arrayBufferToBase64(buf: ArrayBuffer): string {
  const bytes = new Uint8Array(buf);
  let binary = "";
  for (let i = 0; i < bytes.length; i++) {
    binary += String.fromCharCode(bytes[i]);
  }
  return btoa(binary);
}

function base64ToArrayBuffer(b64: string): ArrayBuffer {
  const binary = atob(b64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes.buffer;
}

// --- Crypto helpers ---

async function deriveTransferKey(transferId: string): Promise<CryptoKey> {
  const keyB64 = get(channelKey);
  if (!keyB64) throw new Error("No channel key");

  const keyBytes = Uint8Array.from(atob(keyB64), (c) => c.charCodeAt(0));
  const baseKey = await crypto.subtle.importKey(
    "raw",
    keyBytes,
    "HKDF",
    false,
    ["deriveKey"]
  );

  const info = new TextEncoder().encode(`haven-file-${transferId}`);
  return crypto.subtle.deriveKey(
    { name: "HKDF", hash: "SHA-256", salt: new Uint8Array(32), info },
    baseKey,
    { name: "AES-GCM", length: 256 },
    false,
    ["encrypt", "decrypt"]
  );
}

function makeNonce(chunkIndex: number): Uint8Array {
  const nonce = new Uint8Array(12);
  const view = new DataView(nonce.buffer);
  view.setUint32(4, Math.floor(chunkIndex / 0x100000000));
  view.setUint32(8, chunkIndex >>> 0);
  return nonce;
}

async function encryptChunk(
  key: CryptoKey,
  chunk: Uint8Array,
  chunkIndex: number
): Promise<ArrayBuffer> {
  const nonce = makeNonce(chunkIndex);
  return crypto.subtle.encrypt({ name: "AES-GCM", iv: nonce }, key, chunk);
}

async function decryptChunk(
  key: CryptoKey,
  encrypted: ArrayBuffer,
  chunkIndex: number
): Promise<ArrayBuffer> {
  const nonce = makeNonce(chunkIndex);
  return crypto.subtle.decrypt(
    { name: "AES-GCM", iv: nonce },
    key,
    encrypted
  );
}

// --- Store helpers ---

function updateTransfer(id: string, patch: Partial<Transfer>) {
  transfers.update((list) =>
    list.map((t) => (t.id === id ? { ...t, ...patch } : t))
  );
}

function getTransfer(id: string): Transfer | undefined {
  return get(transfers).find((t) => t.id === id);
}

// --- Public API ---

export function initTransfers(gateway: Gateway) {
  gw = gateway;
}

export function cleanupTransfers() {
  for (const [id, internal] of internals) {
    if (internal.iceTimer) clearTimeout(internal.iceTimer);
    internal.dc?.close();
    internal.pc?.close();
    if (internal.fileHandle) {
      internal.fileHandle.close().catch(() => {});
    }
  }
  internals.clear();
  gw = null;
}

export async function sendFile(
  peerId: string,
  peerUsername: string,
  file: File
) {
  if (!gw) return;

  const transferId = crypto.randomUUID();

  const transfer: Transfer = {
    id: transferId,
    peerId,
    peerUsername,
    filename: file.name,
    size: file.size,
    direction: "send",
    status: "pending",
    bytesTransferred: 0,
  };

  transfers.update((list) => [...list, transfer]);
  internals.set(transferId, { file });

  console.log("[transfers] Sending file offer:", file.name, file.size, "bytes to", peerId);

  gw.send({
    type: "FileOfferSend",
    data: {
      target_user_id: peerId,
      transfer_id: transferId,
      filename: file.name,
      size: file.size,
    },
  });
}

export async function acceptTransfer(transferId: string) {
  if (!gw) return;

  const transfer = getTransfer(transferId);
  if (!transfer || transfer.status !== "pending") return;

  // Open save dialog
  let savePath: string | null;
  try {
    savePath = await save({
      defaultPath: transfer.filename,
      title: "Save file as...",
    });
  } catch (e) {
    console.error("[transfers] Save dialog error:", e);
    updateTransfer(transferId, { status: "failed" });
    return;
  }

  if (!savePath) return; // User cancelled

  updateTransfer(transferId, { status: "connecting" });

  try {
    console.log("[transfers] Creating file at:", savePath);
    const fileHandle = await create(savePath);

    const cryptoKey = await deriveTransferKey(transferId);
    internals.set(transferId, { cryptoKey, fileHandle, relayChunkIndex: 0 });

    console.log("[transfers] Sending accept for transfer:", transferId);
    gw.send({
      type: "FileAcceptSend",
      data: {
        target_user_id: transfer.peerId,
        transfer_id: transferId,
      },
    });
  } catch (e) {
    console.error("[transfers] Failed to accept:", e);
    updateTransfer(transferId, { status: "failed" });
  }
}

export function rejectTransfer(transferId: string) {
  if (!gw) return;

  const transfer = getTransfer(transferId);
  if (!transfer) return;

  updateTransfer(transferId, { status: "rejected" });

  gw.send({
    type: "FileRejectSend",
    data: {
      target_user_id: transfer.peerId,
      transfer_id: transferId,
    },
  });
}

export function cancelTransfer(transferId: string) {
  const internal = internals.get(transferId);
  if (internal) {
    if (internal.iceTimer) clearTimeout(internal.iceTimer);
    internal.dc?.close();
    internal.pc?.close();
    if (internal.fileHandle) {
      internal.fileHandle.close().catch(() => {});
    }
    internals.delete(transferId);
  }
  updateTransfer(transferId, { status: "cancelled" });
}

// --- Gateway event handlers ---

export function handleFileOffer(event: any) {
  const { from_user_id, transfer_id, filename, size } = event.data;
  console.log("[transfers] Received file offer:", filename, size, "bytes from", from_user_id);

  const transfer: Transfer = {
    id: transfer_id,
    peerId: from_user_id,
    peerUsername: from_user_id,
    filename,
    size,
    direction: "receive",
    status: "pending",
    bytesTransferred: 0,
  };

  transfers.update((list) => [...list, transfer]);
}

export async function handleFileAccept(event: any) {
  const { from_user_id, transfer_id } = event.data;

  const transfer = getTransfer(transfer_id);
  if (!transfer || transfer.direction !== "send") return;

  console.log("[transfers] Peer accepted transfer:", transfer_id, "— trying P2P first...");
  updateTransfer(transfer_id, { status: "connecting", startTime: Date.now() });

  try {
    const cryptoKey = await deriveTransferKey(transfer_id);
    const internal = internals.get(transfer_id) || {};
    internal.cryptoKey = cryptoKey;

    // Try WebRTC P2P first
    const pc = new RTCPeerConnection({ iceServers: ICE_SERVERS });
    internal.pc = pc;

    const dc = pc.createDataChannel("file", { ordered: true });
    dc.binaryType = "arraybuffer";
    internal.dc = dc;

    internals.set(transfer_id, internal);

    // Set up ICE timeout — if P2P fails, fall back to relay
    internal.iceTimer = window.setTimeout(() => {
      const t = getTransfer(transfer_id);
      if (t && t.status === "connecting") {
        console.log("[transfers] P2P timed out, falling back to server relay");
        // Clean up WebRTC
        dc.close();
        pc.close();
        internal.pc = undefined;
        internal.dc = undefined;
        internal.relayMode = true;
        // Start relay streaming
        updateTransfer(transfer_id, { status: "transferring" });
        streamFileRelay(transfer_id, from_user_id);
      }
    }, ICE_TIMEOUT);

    // ICE candidate handler
    pc.onicecandidate = (e) => {
      if (e.candidate && gw) {
        gw.send({
          type: "FileSignalSend",
          data: {
            target_user_id: from_user_id,
            transfer_id,
            signal: {
              signal_type: "IceCandidate",
              candidate: e.candidate.candidate,
              sdp_mid: e.candidate.sdpMid,
              sdp_m_line_index: e.candidate.sdpMLineIndex,
            },
          },
        });
      }
    };

    pc.oniceconnectionstatechange = () => {
      console.log("[transfers] ICE state:", pc.iceConnectionState);
      if (pc.iceConnectionState === "failed" || pc.iceConnectionState === "disconnected") {
        // ICE failed — trigger relay fallback immediately
        if (internal.iceTimer) {
          clearTimeout(internal.iceTimer);
          internal.iceTimer = undefined;
        }
        const t = getTransfer(transfer_id);
        if (t && (t.status === "connecting" || t.status === "transferring") && !internal.relayMode) {
          console.log("[transfers] ICE failed, falling back to server relay");
          dc.close();
          pc.close();
          internal.pc = undefined;
          internal.dc = undefined;
          internal.relayMode = true;
          updateTransfer(transfer_id, { status: "transferring" });
          streamFileRelay(transfer_id, from_user_id);
        }
      }
    };

    // When DataChannel opens, start streaming file via P2P
    dc.onopen = () => {
      console.log("[transfers] P2P DataChannel opened!");
      if (internal.iceTimer) {
        clearTimeout(internal.iceTimer);
        internal.iceTimer = undefined;
      }
      updateTransfer(transfer_id, { status: "transferring" });
      streamFile(transfer_id);
    };

    dc.onerror = (e) => {
      console.error("[transfers] DataChannel error:", e);
    };

    // Create and send SDP offer
    const offer = await pc.createOffer();
    await pc.setLocalDescription(offer);

    gw!.send({
      type: "FileSignalSend",
      data: {
        target_user_id: from_user_id,
        transfer_id,
        signal: {
          signal_type: "Offer",
          sdp: offer.sdp,
        },
      },
    });
  } catch (e) {
    console.error("[transfers] Failed to setup sender:", e);
    updateTransfer(transfer_id, { status: "failed" });
  }
}

export function handleFileReject(event: any) {
  const { transfer_id } = event.data;
  updateTransfer(transfer_id, { status: "rejected" });
  const internal = internals.get(transfer_id);
  if (internal) {
    if (internal.iceTimer) clearTimeout(internal.iceTimer);
    internal.dc?.close();
    internal.pc?.close();
    internals.delete(transfer_id);
  }
}

export async function handleFileSignal(event: any) {
  const { from_user_id, transfer_id, signal } = event.data;

  const transfer = getTransfer(transfer_id);
  if (!transfer) return;

  const internal = internals.get(transfer_id) || {};

  if (signal.signal_type === "Offer") {
    // Receiver gets the SDP offer — create answer
    try {
      console.log("[transfers] Received SDP offer, creating answer...");
      const pc = new RTCPeerConnection({ iceServers: ICE_SERVERS });
      internal.pc = pc;
      internals.set(transfer_id, internal);

      pc.onicecandidate = (e) => {
        if (e.candidate && gw) {
          gw.send({
            type: "FileSignalSend",
            data: {
              target_user_id: from_user_id,
              transfer_id,
              signal: {
                signal_type: "IceCandidate",
                candidate: e.candidate.candidate,
                sdp_mid: e.candidate.sdpMid,
                sdp_m_line_index: e.candidate.sdpMLineIndex,
              },
            },
          });
        }
      };

      pc.oniceconnectionstatechange = () => {
        console.log("[transfers] Receiver ICE state:", pc.iceConnectionState);
      };

      // Handle incoming DataChannel (P2P mode)
      pc.ondatachannel = (e) => {
        const dc = e.channel;
        dc.binaryType = "arraybuffer";
        internal.dc = dc;
        console.log("[transfers] Receiver DataChannel opened (P2P mode)");

        let chunkIndex = 0;

        dc.onmessage = async (msg) => {
          if (typeof msg.data === "string" && msg.data === "DONE") {
            if (internal.fileHandle) {
              await internal.fileHandle.close();
            }
            updateTransfer(transfer_id, { status: "completed" });
            pc.close();
            internals.delete(transfer_id);
            return;
          }

          try {
            if (!internal.cryptoKey) return;
            const decrypted = await decryptChunk(
              internal.cryptoKey,
              msg.data as ArrayBuffer,
              chunkIndex
            );
            chunkIndex++;

            if (internal.fileHandle) {
              await internal.fileHandle.write(new Uint8Array(decrypted));
            }

            const t = getTransfer(transfer_id);
            if (t) {
              updateTransfer(transfer_id, {
                bytesTransferred: t.bytesTransferred + decrypted.byteLength,
                status: "transferring",
              });
            }
          } catch (e) {
            console.error("[transfers] Failed to write chunk:", e);
            updateTransfer(transfer_id, { status: "failed" });
            pc.close();
          }
        };

        dc.onerror = (e) => {
          console.error("[transfers] Receiver DC error:", e);
        };
      };

      await pc.setRemoteDescription(
        new RTCSessionDescription({ type: "offer", sdp: signal.sdp })
      );
      const answer = await pc.createAnswer();
      await pc.setLocalDescription(answer);

      gw!.send({
        type: "FileSignalSend",
        data: {
          target_user_id: from_user_id,
          transfer_id,
          signal: {
            signal_type: "Answer",
            sdp: answer.sdp,
          },
        },
      });
    } catch (e) {
      console.error("[transfers] Failed to handle SDP offer:", e);
      updateTransfer(transfer_id, { status: "failed" });
    }
  } else if (signal.signal_type === "Answer") {
    try {
      if (internal.pc) {
        await internal.pc.setRemoteDescription(
          new RTCSessionDescription({ type: "answer", sdp: signal.sdp })
        );
      }
    } catch (e) {
      console.error("[transfers] Failed to handle SDP answer:", e);
    }
  } else if (signal.signal_type === "IceCandidate") {
    try {
      if (internal.pc) {
        await internal.pc.addIceCandidate(
          new RTCIceCandidate({
            candidate: signal.candidate,
            sdpMid: signal.sdp_mid,
            sdpMLineIndex: signal.sdp_m_line_index,
          })
        );
      }
    } catch (e) {
      console.error("[transfers] Failed to add ICE candidate:", e);
    }
  }
}

// --- Relay event handlers (server-relayed chunks) ---

export async function handleFileChunk(event: any) {
  const { from_user_id, transfer_id, chunk_index, data } = event.data;

  const internal = internals.get(transfer_id);
  if (!internal?.cryptoKey || !internal.fileHandle) return;

  try {
    const encrypted = base64ToArrayBuffer(data);
    const decrypted = await decryptChunk(internal.cryptoKey, encrypted, chunk_index);

    await internal.fileHandle.write(new Uint8Array(decrypted));

    const t = getTransfer(transfer_id);
    if (t) {
      updateTransfer(transfer_id, {
        bytesTransferred: t.bytesTransferred + decrypted.byteLength,
        status: "transferring",
        startTime: t.startTime || Date.now(),
      });
    }
  } catch (e) {
    console.error("[transfers] Relay chunk write error:", e);
    updateTransfer(transfer_id, { status: "failed" });
  }
}

export async function handleFileDone(event: any) {
  const { from_user_id, transfer_id } = event.data;

  const internal = internals.get(transfer_id);
  if (internal?.fileHandle) {
    await internal.fileHandle.close();
  }
  internals.delete(transfer_id);
  updateTransfer(transfer_id, { status: "completed" });
  console.log("[transfers] Relay transfer complete:", transfer_id);
}

// --- File streaming via P2P DataChannel (sender side) ---

async function streamFile(transferId: string) {
  const internal = internals.get(transferId);
  if (!internal?.file || !internal.dc || !internal.cryptoKey) return;

  const dc = internal.dc;
  const key = internal.cryptoKey;
  const reader = internal.file.stream().getReader();
  let chunkIndex = 0;
  let bytesSent = 0;
  let leftover = new Uint8Array(0);

  try {
    while (true) {
      const { done, value } = await reader.read();

      if (done) {
        if (leftover.length > 0) {
          const encrypted = await encryptChunk(key, leftover, chunkIndex);
          await waitForBuffer(dc);
          dc.send(encrypted);
          bytesSent += leftover.length;
          chunkIndex++;
          updateTransfer(transferId, { bytesTransferred: bytesSent });
        }
        dc.send("DONE");
        updateTransfer(transferId, {
          status: "completed",
          bytesTransferred: bytesSent,
        });
        setTimeout(() => {
          dc.close();
          internal.pc?.close();
          internals.delete(transferId);
        }, 1000);
        return;
      }

      let data: Uint8Array;
      if (leftover.length > 0) {
        data = new Uint8Array(leftover.length + value.length);
        data.set(leftover, 0);
        data.set(value, leftover.length);
        leftover = new Uint8Array(0);
      } else {
        data = value;
      }

      let offset = 0;
      while (offset + CHUNK_SIZE <= data.length) {
        const chunk = data.slice(offset, offset + CHUNK_SIZE);
        const encrypted = await encryptChunk(key, chunk, chunkIndex);
        await waitForBuffer(dc);
        dc.send(encrypted);
        bytesSent += chunk.length;
        chunkIndex++;
        offset += CHUNK_SIZE;

        if (chunkIndex % 16 === 0) {
          updateTransfer(transferId, { bytesTransferred: bytesSent });
        }
      }

      if (offset < data.length) {
        leftover = data.slice(offset);
      }
    }
  } catch (e) {
    console.error("[transfers] P2P stream error:", e);
    updateTransfer(transferId, { status: "failed" });
  }
}

// --- File streaming via server relay (fallback) ---

async function streamFileRelay(transferId: string, targetUserId: string) {
  const internal = internals.get(transferId);
  if (!internal?.file || !internal.cryptoKey || !gw) return;

  console.log("[transfers] Starting relay stream for:", transferId);
  const key = internal.cryptoKey;
  const reader = internal.file.stream().getReader();
  let chunkIndex = 0;
  let bytesSent = 0;
  let leftover = new Uint8Array(0);

  try {
    while (true) {
      const { done, value } = await reader.read();

      if (done) {
        if (leftover.length > 0) {
          const encrypted = await encryptChunk(key, leftover, chunkIndex);
          const b64 = arrayBufferToBase64(encrypted);
          gw!.send({
            type: "FileChunkSend",
            data: {
              target_user_id: targetUserId,
              transfer_id: transferId,
              chunk_index: chunkIndex,
              data: b64,
            },
          });
          bytesSent += leftover.length;
          chunkIndex++;
          updateTransfer(transferId, { bytesTransferred: bytesSent });
        }

        // Signal completion
        gw!.send({
          type: "FileDoneSend",
          data: {
            target_user_id: targetUserId,
            transfer_id: transferId,
          },
        });
        updateTransfer(transferId, {
          status: "completed",
          bytesTransferred: bytesSent,
        });
        internals.delete(transferId);
        console.log("[transfers] Relay send complete:", transferId);
        return;
      }

      let data: Uint8Array;
      if (leftover.length > 0) {
        data = new Uint8Array(leftover.length + value.length);
        data.set(leftover, 0);
        data.set(value, leftover.length);
        leftover = new Uint8Array(0);
      } else {
        data = value;
      }

      let offset = 0;
      while (offset + RELAY_CHUNK_SIZE <= data.length) {
        const chunk = data.slice(offset, offset + RELAY_CHUNK_SIZE);
        const encrypted = await encryptChunk(key, chunk, chunkIndex);
        const b64 = arrayBufferToBase64(encrypted);

        gw!.send({
          type: "FileChunkSend",
          data: {
            target_user_id: targetUserId,
            transfer_id: transferId,
            chunk_index: chunkIndex,
            data: b64,
          },
        });

        bytesSent += chunk.length;
        chunkIndex++;
        offset += RELAY_CHUNK_SIZE;

        if (chunkIndex % 16 === 0) {
          updateTransfer(transferId, { bytesTransferred: bytesSent });
        }

        // Yield to event loop periodically to avoid blocking
        if (chunkIndex % 4 === 0) {
          await new Promise((r) => setTimeout(r, 0));
        }
      }

      if (offset < data.length) {
        leftover = data.slice(offset);
      }
    }
  } catch (e) {
    console.error("[transfers] Relay stream error:", e);
    updateTransfer(transferId, { status: "failed" });
  }
}

function waitForBuffer(dc: RTCDataChannel): Promise<void> {
  if (dc.bufferedAmount < BUFFER_HIGH) {
    return Promise.resolve();
  }
  return new Promise((resolve) => {
    dc.bufferedAmountLowThreshold = BUFFER_LOW;
    dc.onbufferedamountlow = () => {
      dc.onbufferedamountlow = null;
      resolve();
    };
  });
}

// --- Utility ---

export function formatBytes(bytes: number): string {
  if (bytes === 0) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + " " + sizes[i];
}

export function formatSpeed(
  bytesTransferred: number,
  startTime?: number
): string {
  if (!startTime) return "";
  const elapsed = (Date.now() - startTime) / 1000;
  if (elapsed < 0.1) return "";
  return formatBytes(bytesTransferred / elapsed) + "/s";
}
