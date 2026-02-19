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
const STUN_SERVERS: RTCIceServer[] = [
  { urls: "stun:stun.l.google.com:19302" },
  { urls: "stun:stun1.l.google.com:19302" },
];

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
  // chunk index as big-endian u64 at offset 4 (first 4 bytes zero)
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
  // Tear down all active connections
  for (const [id, internal] of internals) {
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
  const userId = get(auth).userId!;

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

  // Send offer via gateway signaling
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
  const savePath = await save({
    defaultPath: transfer.filename,
    title: "Save file as...",
  });

  if (!savePath) return; // User cancelled

  updateTransfer(transferId, { status: "connecting" });

  try {
    // Create file handle for streaming writes
    const fileHandle = await create(savePath);

    const cryptoKey = await deriveTransferKey(transferId);
    internals.set(transferId, { cryptoKey, fileHandle });

    // Send accept signal
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

  // Look up username from the event — gateway doesn't send it, so use userId as fallback
  // The FileChannel component can resolve this from the presence store
  const transfer: Transfer = {
    id: transfer_id,
    peerId: from_user_id,
    peerUsername: from_user_id, // Will be resolved by UI from presence store
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

  updateTransfer(transfer_id, { status: "connecting", startTime: Date.now() });

  try {
    const cryptoKey = await deriveTransferKey(transfer_id);
    const internal = internals.get(transfer_id) || {};
    internal.cryptoKey = cryptoKey;

    // Create WebRTC peer connection (sender is the offerer)
    const pc = new RTCPeerConnection({ iceServers: STUN_SERVERS });
    internal.pc = pc;

    // Create DataChannel — reliable ordered
    const dc = pc.createDataChannel("file", {
      ordered: true,
    });
    dc.binaryType = "arraybuffer";
    internal.dc = dc;

    internals.set(transfer_id, internal);

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

    // When DataChannel opens, start streaming file
    dc.onopen = () => {
      updateTransfer(transfer_id, { status: "transferring" });
      streamFile(transfer_id);
    };

    dc.onerror = (e) => {
      console.error("[transfers] DataChannel error:", e);
      updateTransfer(transfer_id, { status: "failed" });
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
    console.error("[transfers] Failed to setup sender WebRTC:", e);
    updateTransfer(transfer_id, { status: "failed" });
  }
}

export function handleFileReject(event: any) {
  const { transfer_id } = event.data;
  updateTransfer(transfer_id, { status: "rejected" });
  const internal = internals.get(transfer_id);
  if (internal) {
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
      const pc = new RTCPeerConnection({ iceServers: STUN_SERVERS });
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

      // Handle incoming DataChannel
      pc.ondatachannel = (e) => {
        const dc = e.channel;
        dc.binaryType = "arraybuffer";
        internal.dc = dc;

        let chunkIndex = 0;

        dc.onmessage = async (msg) => {
          // Check for completion sentinel
          if (typeof msg.data === "string" && msg.data === "DONE") {
            if (internal.fileHandle) {
              await internal.fileHandle.close();
            }
            updateTransfer(transfer_id, { status: "completed" });
            pc.close();
            internals.delete(transfer_id);
            return;
          }

          // Decrypt and write chunk to disk
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
          updateTransfer(transfer_id, { status: "failed" });
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
    // Sender gets the SDP answer
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
    // Both sides handle ICE candidates
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

// --- File streaming (sender side) ---

async function streamFile(transferId: string) {
  const internal = internals.get(transferId);
  if (!internal?.file || !internal.dc || !internal.cryptoKey) return;

  const dc = internal.dc;
  const key = internal.cryptoKey;
  const reader = internal.file.stream().getReader();
  let chunkIndex = 0;
  let bytesSent = 0;

  // Buffer for accumulating partial chunks from the stream
  let leftover = new Uint8Array(0);

  try {
    while (true) {
      const { done, value } = await reader.read();

      if (done) {
        // Send any remaining leftover
        if (leftover.length > 0) {
          const encrypted = await encryptChunk(key, leftover, chunkIndex);
          await waitForBuffer(dc);
          dc.send(encrypted);
          bytesSent += leftover.length;
          chunkIndex++;
          updateTransfer(transferId, { bytesTransferred: bytesSent });
        }
        // Send completion sentinel
        dc.send("DONE");
        updateTransfer(transferId, {
          status: "completed",
          bytesTransferred: bytesSent,
        });

        // Clean up
        setTimeout(() => {
          dc.close();
          internal.pc?.close();
          internals.delete(transferId);
        }, 1000);
        return;
      }

      // Combine leftover with new data
      let data: Uint8Array;
      if (leftover.length > 0) {
        data = new Uint8Array(leftover.length + value.length);
        data.set(leftover, 0);
        data.set(value, leftover.length);
        leftover = new Uint8Array(0);
      } else {
        data = value;
      }

      // Send full chunks
      let offset = 0;
      while (offset + CHUNK_SIZE <= data.length) {
        const chunk = data.slice(offset, offset + CHUNK_SIZE);
        const encrypted = await encryptChunk(key, chunk, chunkIndex);

        await waitForBuffer(dc);
        dc.send(encrypted);

        bytesSent += chunk.length;
        chunkIndex++;
        offset += CHUNK_SIZE;

        // Update progress periodically (every 16 chunks ~= 1MB)
        if (chunkIndex % 16 === 0) {
          updateTransfer(transferId, { bytesTransferred: bytesSent });
        }
      }

      // Save remainder for next iteration
      if (offset < data.length) {
        leftover = data.slice(offset);
      }
    }
  } catch (e) {
    console.error("[transfers] Stream error:", e);
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

export function formatSpeed(bytesTransferred: number, startTime?: number): string {
  if (!startTime) return "";
  const elapsed = (Date.now() - startTime) / 1000;
  if (elapsed < 0.1) return "";
  return formatBytes(bytesTransferred / elapsed) + "/s";
}
