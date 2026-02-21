import { writable, derived, get } from "svelte/store";
import type { Gateway } from "../ipc/gateway";
import { create, type FileHandle } from "@tauri-apps/plugin-fs";
import { save } from "@tauri-apps/plugin-dialog";
import { channelKey } from "./messages";
import { auth } from "./auth";
import { CryptoWorkerPool } from "../workers/crypto-pool";

const cryptoPool = new CryptoWorkerPool();

// --- Constants ---

const CHUNK_SIZE = 64 * 1024; // 64KB — WebRTC DataChannel safe max
const BUFFER_HIGH = 1 * 1024 * 1024; // 1MB — back-pressure threshold
const BUFFER_LOW = 512 * 1024; // 512KB — resume threshold
function getTurnServers(): RTCIceServer[] {
  try {
    const config = localStorage.getItem("haven_turn_servers");
    if (config) return JSON.parse(config);
  } catch {}
  return [];
}

const ICE_SERVERS: RTCIceServer[] = [
  { urls: "stun:stun.l.google.com:19302" },
  { urls: "stun:stun1.l.google.com:19302" },
  // TURN servers — configurable via localStorage key "haven_turn_servers"
  ...getTurnServers(),
];
const ICE_TIMEOUT = 10_000; // 10s — if P2P fails, fall back to server relay
const RELAY_ACK_INTERVAL = 20; // receiver acks every N chunks

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
  // Ack-based flow control for relay sender
  ackedChunkIndex?: number; // highest acked chunk index (-1 = none)
  ackResolve?: (() => void) | null; // resolve function when waiting for ack
  writeQueue?: Promise<void>; // sequential write chain — prevents race between chunks and FileDone
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

// --- UUID binary helpers (for binary WebSocket protocol) ---

function uuidToBytes(uuid: string): Uint8Array {
    const hex = uuid.replace(/-/g, "");
    const bytes = new Uint8Array(16);
    for (let i = 0; i < 16; i++) {
        bytes[i] = parseInt(hex.substr(i * 2, 2), 16);
    }
    return bytes;
}

function bytesToUuid(bytes: Uint8Array): string {
    let hex = "";
    for (let i = 0; i < 16; i++) {
        hex += bytes[i].toString(16).padStart(2, "0");
    }
    return `${hex.slice(0,8)}-${hex.slice(8,12)}-${hex.slice(12,16)}-${hex.slice(16,20)}-${hex.slice(20)}`;
}

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
  return bytes.buffer as ArrayBuffer;
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
  const view = new DataView(nonce.buffer as ArrayBuffer);
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
  return crypto.subtle.encrypt({ name: "AES-GCM", iv: nonce as unknown as Uint8Array<ArrayBuffer> }, key, chunk as unknown as Uint8Array<ArrayBuffer>);
}

async function decryptChunk(
  key: CryptoKey,
  encrypted: ArrayBuffer,
  chunkIndex: number
): Promise<ArrayBuffer> {
  const nonce = makeNonce(chunkIndex);
  return crypto.subtle.decrypt(
    { name: "AES-GCM", iv: nonce as unknown as Uint8Array<ArrayBuffer> },
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
  // Drain the scheduler so it stops sending chunks
  for (const [id] of internals) {
    scheduler.removeTransfer(id);
  }

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
  // Remove from relay scheduler if active
  scheduler.removeTransfer(transferId);

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

    // Send ack every RELAY_ACK_INTERVAL chunks for flow control
    if (chunk_index > 0 && chunk_index % RELAY_ACK_INTERVAL === 0 && gw) {
      gw.send({
        type: "FileAckSend",
        data: {
          target_user_id: from_user_id,
          transfer_id,
          ack_chunk_index: chunk_index,
        },
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

  // Send final ack so sender knows receiver is done
  if (gw) {
    gw.send({
      type: "FileAckSend",
      data: {
        target_user_id: from_user_id,
        transfer_id,
        ack_chunk_index: 0xFFFFFFFF, // sentinel: all done
      },
    });
  }

  internals.delete(transfer_id);
  updateTransfer(transfer_id, { status: "completed" });
  console.log("[transfers] Relay transfer complete:", transfer_id);
}

export function handleFileAck(event: any) {
  const { transfer_id, ack_chunk_index } = event.data;

  // Route to centralized scheduler for relay transfers
  scheduler.onAck(transfer_id, ack_chunk_index);

  // Also handle legacy per-transfer acks (P2P fallback path, if ever used)
  const internal = internals.get(transfer_id);
  if (!internal) return;

  internal.ackedChunkIndex = ack_chunk_index;

  if (internal.ackResolve) {
    internal.ackResolve();
    internal.ackResolve = null;
  }
}

// --- Binary WebSocket message handler (primary fast path) ---

export function handleBinaryMessage(data: ArrayBuffer) {
    const view = new DataView(data);
    const bytes = new Uint8Array(data);
    if (data.byteLength < 1) return;

    const msgType = bytes[0];

    if (msgType === 0x01) {
        // FileChunk: [type(1)][from_uid(16)][transfer_id(16)][chunk_idx(4)][payload...]
        if (data.byteLength < 37) return;
        const fromUserId = bytesToUuid(bytes.slice(1, 17));
        const transferId = bytesToUuid(bytes.slice(17, 33));
        const chunkIndex = view.getUint32(33);
        const payload = data.slice(37);

        handleBinaryFileChunk(fromUserId, transferId, chunkIndex, payload);
    } else if (msgType === 0x02) {
        // FileAck: [type(1)][from_uid(16)][transfer_id(16)][ack_chunk_idx(4)]
        if (data.byteLength < 37) return;
        const transferId = bytesToUuid(bytes.slice(17, 33));
        const ackChunkIndex = view.getUint32(33);

        scheduler.onAck(transferId, ackChunkIndex);
        // Also handle legacy path
        const internal = internals.get(transferId);
        if (internal) {
            internal.ackedChunkIndex = ackChunkIndex;
            if (internal.ackResolve) {
                internal.ackResolve();
                internal.ackResolve = null;
            }
        }
    } else if (msgType === 0x03) {
        // FileDone: [type(1)][from_uid(16)][transfer_id(16)]
        if (data.byteLength < 33) return;
        const fromUserId = bytesToUuid(bytes.slice(1, 17));
        const transferId = bytesToUuid(bytes.slice(17, 33));

        handleBinaryFileDone(fromUserId, transferId);
    }
}

async function handleBinaryFileChunk(
    fromUserId: string,
    transferId: string,
    chunkIndex: number,
    encryptedPayload: ArrayBuffer
) {
    const internal = internals.get(transferId);
    if (!internal?.cryptoKey || !internal.fileHandle) return;

    // Start decryption immediately (runs in parallel across worker pool)
    const decryptPromise = cryptoPool.decrypt(
        internal.cryptoKey,
        encryptedPayload,
        chunkIndex
    );

    // Chain the file write sequentially — prevents interleaved writes and
    // ensures FileDone can await all pending writes before closing the handle.
    const prevQueue = internal.writeQueue || Promise.resolve();
    internal.writeQueue = prevQueue.then(async () => {
        try {
            const decrypted = await decryptPromise;
            await internal.fileHandle!.write(new Uint8Array(decrypted));

            const t = getTransfer(transferId);
            if (t) {
                updateTransfer(transferId, {
                    bytesTransferred: t.bytesTransferred + decrypted.byteLength,
                    status: "transferring",
                    startTime: t.startTime || Date.now(),
                });
            }
        } catch (e) {
            console.error("[transfers] Binary chunk write error:", e);
            updateTransfer(transferId, { status: "failed" });
        }
    });

    // Send binary ack every RELAY_ACK_INTERVAL chunks
    if (chunkIndex > 0 && chunkIndex % RELAY_ACK_INTERVAL === 0 && gw) {
        const targetBytes = uuidToBytes(fromUserId);
        const transferBytes = uuidToBytes(transferId);
        const ackFrame = new Uint8Array(37);
        ackFrame[0] = 0x02; // FileAckSend
        ackFrame.set(targetBytes, 1);
        ackFrame.set(transferBytes, 17);
        const ackView = new DataView(ackFrame.buffer as ArrayBuffer);
        ackView.setUint32(33, chunkIndex);
        gw.sendBinary(ackFrame.buffer as ArrayBuffer);
    }
}

async function handleBinaryFileDone(fromUserId: string, transferId: string) {
    const internal = internals.get(transferId);

    // Wait for all queued chunk writes to finish before closing the file.
    // Without this, FileDone races with in-flight decrypts and the last
    // chunks fail to write → status incorrectly flips to "failed".
    if (internal?.writeQueue) {
        await internal.writeQueue;
    }

    if (internal?.fileHandle) {
        await internal.fileHandle.close();
    }

    // Send binary final ack
    if (gw) {
        const targetBytes = uuidToBytes(fromUserId);
        const transferBytes = uuidToBytes(transferId);
        const ackFrame = new Uint8Array(37);
        ackFrame[0] = 0x02;
        ackFrame.set(targetBytes, 1);
        ackFrame.set(transferBytes, 17);
        const ackView = new DataView(ackFrame.buffer as ArrayBuffer);
        ackView.setUint32(33, 0xFFFFFFFF); // sentinel
        gw.sendBinary(ackFrame.buffer as ArrayBuffer);
    }

    internals.delete(transferId);
    updateTransfer(transferId, { status: "completed" });
    console.log("[transfers] Binary relay transfer complete:", transferId);
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

// --- Centralized Transfer Scheduler (relay mode) ---
//
// Instead of each relay transfer running its own independent async loop
// that competes for the same WebSocket, this scheduler round-robins chunks
// across all active relay transfers through a single loop.  This eliminates
// the "flip-flopping" burst pattern and gives each transfer fair bandwidth.

interface RelayStream {
  transferId: string;
  targetUserId: string;
  reader: ReadableStreamDefaultReader<Uint8Array>;
  cryptoKey: CryptoKey;
  chunkIndex: number;
  bytesSent: number;
  totalSize: number;
  leftover: Uint8Array;
  done: boolean;
}

class TransferScheduler {
  private streams: Map<string, RelayStream> = new Map();
  private running = false;
  private globalAcked = new Map<string, number>(); // transferId -> highest acked chunk
  private ackResolvers = new Map<string, (() => void) | null>();

  // Shared window across ALL transfers
  private static readonly GLOBAL_WINDOW = 200;
  private static readonly CHUNK_SIZE = 256 * 1024;
  private static readonly ACK_INTERVAL = 20;
  private static readonly BURST_SIZE = 16;

  addTransfer(
    transferId: string,
    targetUserId: string,
    file: File,
    cryptoKey: CryptoKey
  ) {
    const stream: RelayStream = {
      transferId,
      targetUserId,
      reader: file.stream().getReader(),
      cryptoKey,
      chunkIndex: 0,
      bytesSent: 0,
      totalSize: file.size,
      leftover: new Uint8Array(0),
      done: false,
    };
    this.streams.set(transferId, stream);
    this.globalAcked.set(transferId, -1);

    if (!this.running) {
      this.running = true;
      this.run();
    }
  }

  removeTransfer(transferId: string) {
    const stream = this.streams.get(transferId);
    if (stream) {
      // Cancel the reader so we don't leak the underlying stream
      stream.reader.cancel().catch(() => {});
    }
    this.streams.delete(transferId);
    this.globalAcked.delete(transferId);
    const resolver = this.ackResolvers.get(transferId);
    if (resolver) {
      resolver();
    }
    this.ackResolvers.delete(transferId);
  }

  onAck(transferId: string, ackChunkIndex: number) {
    this.globalAcked.set(transferId, ackChunkIndex);
    const resolver = this.ackResolvers.get(transferId);
    if (resolver) {
      resolver();
      this.ackResolvers.set(transferId, null);
    }
  }

  private totalInFlight(): number {
    let total = 0;
    for (const [id, stream] of this.streams) {
      const acked = this.globalAcked.get(id) ?? -1;
      total += stream.chunkIndex - acked - 1;
    }
    return total;
  }

  private async run() {
    while (this.streams.size > 0) {
      let anyProgress = false;

      // Round-robin: send a burst of chunks from each active transfer
      for (const [id, stream] of this.streams) {
        if (stream.done) continue;

        // Wait for window space (block until space opens, don't skip)
        while (this.totalInFlight() >= TransferScheduler.GLOBAL_WINDOW) {
          await new Promise<void>((resolve) => {
            let maxInFlight = -1;
            let maxId = id;
            for (const [tid, s] of this.streams) {
              const acked = this.globalAcked.get(tid) ?? -1;
              const inFlight = s.chunkIndex - acked - 1;
              if (inFlight > maxInFlight) {
                maxInFlight = inFlight;
                maxId = tid;
              }
            }
            this.ackResolvers.set(maxId, resolve);
          });
        }

        // Send a burst of chunks (read + encrypt in parallel, then send)
        const sent = await this.sendBurst(stream);
        if (sent > 0) {
          anyProgress = true;
          updateTransfer(stream.transferId, {
            bytesTransferred: stream.bytesSent,
          });
        }

        // Check if this stream finished reading
        if (stream.done) {
          if (gw) {
            const targetBytes = uuidToBytes(stream.targetUserId);
            const transferBytes = uuidToBytes(stream.transferId);
            const frame = new Uint8Array(33);
            frame[0] = 0x03; // FileDoneSend
            frame.set(targetBytes, 1);
            frame.set(transferBytes, 17);
            gw.sendBinary(frame.buffer as ArrayBuffer);
          }
          updateTransfer(stream.transferId, {
            status: "completed",
            bytesTransferred: stream.bytesSent,
          });
          this.streams.delete(id);
          internals.delete(stream.transferId);
          console.log("[transfers] Relay send complete (scheduler):", stream.transferId);
        }
      }

      // Yield to event loop so acks and other events can be processed
      if (anyProgress) {
        await new Promise((r) => setTimeout(r, 0));
      } else {
        await new Promise((r) => setTimeout(r, 10));
      }
    }

    this.running = false;
  }

  /** Read up to BURST_SIZE chunks, encrypt all in parallel, send all frames. */
  private async sendBurst(stream: RelayStream): Promise<number> {
    const batch: { buf: ArrayBuffer; len: number; idx: number }[] = [];

    for (let i = 0; i < TransferScheduler.BURST_SIZE; i++) {
      // Stop if we'd exceed the window
      if (this.totalInFlight() >= TransferScheduler.GLOBAL_WINDOW) break;

      const chunk = await this.readNextChunk(stream);
      if (!chunk) {
        stream.done = true;
        break;
      }

      const buf = chunk.buffer.slice(
        chunk.byteOffset,
        chunk.byteOffset + chunk.byteLength
      ) as ArrayBuffer;
      batch.push({ buf, len: chunk.byteLength, idx: stream.chunkIndex });
      stream.chunkIndex++;
    }

    if (batch.length === 0) return 0;

    // Encrypt all chunks in parallel across the worker pool
    const encrypted = await Promise.all(
      batch.map((c) => cryptoPool.encrypt(stream.cryptoKey, c.buf, c.idx))
    );

    // Build and send all binary frames
    const targetBytes = uuidToBytes(stream.targetUserId);
    const transferBytes = uuidToBytes(stream.transferId);

    for (let i = 0; i < encrypted.length; i++) {
      if (!gw) break;
      const frame = new Uint8Array(37 + encrypted[i].byteLength);
      frame[0] = 0x01; // FileChunkSend
      frame.set(targetBytes, 1);
      frame.set(transferBytes, 17);
      const view = new DataView(frame.buffer as ArrayBuffer);
      view.setUint32(33, batch[i].idx);
      frame.set(new Uint8Array(encrypted[i]), 37);
      gw.sendBinary(frame.buffer as ArrayBuffer);
      stream.bytesSent += batch[i].len;
    }

    return encrypted.length;
  }

  private async readNextChunk(
    stream: RelayStream
  ): Promise<Uint8Array | null> {
    const CHUNK_SIZE = TransferScheduler.CHUNK_SIZE;

    // If we have enough leftover, use it directly
    if (stream.leftover.length >= CHUNK_SIZE) {
      const chunk = stream.leftover.slice(0, CHUNK_SIZE);
      stream.leftover = stream.leftover.slice(CHUNK_SIZE);
      return chunk;
    }

    // Read more data from the file stream
    const { done, value } = await stream.reader.read();

    if (done) {
      // Return remaining leftover if any
      if (stream.leftover.length > 0) {
        const last = stream.leftover;
        stream.leftover = new Uint8Array(0);
        return last;
      }
      return null;
    }

    // Combine leftover + new data
    let data: Uint8Array;
    if (stream.leftover.length > 0) {
      data = new Uint8Array(stream.leftover.length + value.length);
      data.set(stream.leftover, 0);
      data.set(value, stream.leftover.length);
    } else {
      data = value;
    }

    if (data.length >= CHUNK_SIZE) {
      const chunk = data.slice(0, CHUNK_SIZE);
      stream.leftover = data.slice(CHUNK_SIZE);
      return chunk;
    } else {
      stream.leftover = data;
      // Recurse to accumulate more data until we have a full chunk
      return this.readNextChunk(stream);
    }
  }
}

const scheduler = new TransferScheduler();

// --- File streaming via server relay (fallback) ---
//
// Thin wrapper: instead of running its own loop, just registers with the
// centralized scheduler so all relay transfers share a single send loop.

async function streamFileRelay(transferId: string, targetUserId: string) {
  const internal = internals.get(transferId);
  if (!internal?.file || !internal.cryptoKey || !gw) return;

  console.log("[transfers] Adding to relay scheduler:", transferId);
  internal.relayMode = true;
  scheduler.addTransfer(
    transferId,
    targetUserId,
    internal.file,
    internal.cryptoKey
  );
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
