import { writable, derived, get } from "svelte/store";
import type { Gateway } from "../ipc/gateway";
import { channelKey } from "./messages";

// --- Types ---

export interface VoiceParticipant {
  userId: string;
  username: string;
  sessionId: string | null;
  selfMute: boolean;
  selfDeaf: boolean;
  speaking: boolean;
}

// --- Constants ---

const VOICE_CHANNEL_ID = "00000000-0000-0000-0000-000000000002";
const CAPTURE_INTERVAL_MS = 60; // ~60ms chunks for low latency

// --- Stores ---

export const voiceConnected = writable(false);
export const voiceMuted = writable(false);
export const voiceDeafened = writable(false);
export const voiceParticipants = writable<Map<string, VoiceParticipant>>(new Map());
export const localSpeaking = writable(false);

export const voiceError = writable<string | null>(null);

export const voiceParticipantList = derived(voiceParticipants, ($p) =>
  Array.from($p.values())
);

// --- Module state ---

let gw: Gateway | null = null;
let myUserId: string | null = null;
let localStream: MediaStream | null = null;
let audioContext: AudioContext | null = null;
let captureNode: ScriptProcessorNode | null = null;
let vadInterval: number | null = null;

// E2E encryption key (derived from channel key)
let cryptoKey: CryptoKey | null = null;

// Playback state per remote user
const playbackContexts: Map<string, {
  ctx: AudioContext;
  nextPlayTime: number;
}> = new Map();

// --- E2E Crypto helpers ---

async function importCryptoKey(): Promise<boolean> {
  const keyB64 = get(channelKey);
  if (!keyB64) {
    console.error("[voice] No channel key available for E2E encryption");
    return false;
  }
  const keyBytes = Uint8Array.from(atob(keyB64), (c) => c.charCodeAt(0));
  cryptoKey = await crypto.subtle.importKey(
    "raw",
    keyBytes,
    { name: "AES-GCM" },
    false,
    ["encrypt", "decrypt"]
  );
  return true;
}

async function encryptAudio(pcmBytes: Uint8Array): Promise<string> {
  if (!cryptoKey) return "";
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const ciphertext = await crypto.subtle.encrypt(
    { name: "AES-GCM", iv },
    cryptoKey,
    pcmBytes
  );
  // Prepend IV (12 bytes) to ciphertext, then base64 encode
  const combined = new Uint8Array(12 + ciphertext.byteLength);
  combined.set(iv, 0);
  combined.set(new Uint8Array(ciphertext), 12);
  let binary = "";
  for (let i = 0; i < combined.length; i++) {
    binary += String.fromCharCode(combined[i]);
  }
  return btoa(binary);
}

async function decryptAudio(b64: string): Promise<Uint8Array | null> {
  if (!cryptoKey) return null;
  const raw = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
  if (raw.length < 13) return null; // IV (12) + at least 1 byte
  const iv = raw.slice(0, 12);
  const ciphertext = raw.slice(12);
  try {
    const plaintext = await crypto.subtle.decrypt(
      { name: "AES-GCM", iv },
      cryptoKey,
      ciphertext
    );
    return new Uint8Array(plaintext);
  } catch {
    return null;
  }
}

// --- Public API ---

export function initVoice(gateway: Gateway, userId: string) {
  gw = gateway;
  myUserId = userId;
}

export async function joinVoice() {
  voiceError.set(null);

  if (!gw || !myUserId) {
    console.error("[voice] not initialized — gw:", !!gw, "myUserId:", myUserId);
    voiceError.set("Voice not initialized. Try reconnecting.");
    return;
  }

  if (!navigator.mediaDevices?.getUserMedia) {
    console.error("[voice] getUserMedia not available");
    voiceError.set("Microphone API not available in this context.");
    return;
  }

  try {
    localStream = await navigator.mediaDevices.getUserMedia({ audio: true });
  } catch (e: any) {
    console.error("[voice] getUserMedia failed:", e);
    voiceError.set("Microphone access denied: " + (e.message || e.name));
    return;
  }

  // Import E2E encryption key
  const hasKey = await importCryptoKey();
  if (!hasKey) {
    voiceError.set("No encryption key set. Set a channel key first.");
    localStream.getTracks().forEach((t) => t.stop());
    localStream = null;
    return;
  }

  voiceConnected.set(true);
  voiceMuted.set(false);
  voiceDeafened.set(false);

  // Set up audio capture and send to server
  setupAudioCapture(localStream);
  setupVAD(localStream);

  gw.send({
    type: "VoiceJoin",
    data: { channel_id: VOICE_CHANNEL_ID },
  });

}

export function leaveVoice() {
  if (!gw) return;

  gw.send({ type: "VoiceLeave" });

  cleanupLocalMedia();
  cleanupAllPlayback();

  voiceConnected.set(false);
  voiceMuted.set(false);
  voiceDeafened.set(false);
  voiceParticipants.set(new Map());
}

export function toggleMute() {
  if (!localStream || !gw) return;

  const muted = !get(voiceMuted);
  voiceMuted.set(muted);

  for (const track of localStream.getAudioTracks()) {
    track.enabled = !muted;
  }

  gw.send({
    type: "VoiceStateSet",
    data: { self_mute: muted, self_deaf: get(voiceDeafened) },
  });
}

export function toggleDeafen() {
  if (!gw) return;

  const deafened = !get(voiceDeafened);
  voiceDeafened.set(deafened);

  // When deafening, also mute
  if (deafened && !get(voiceMuted)) {
    voiceMuted.set(true);
    if (localStream) {
      for (const track of localStream.getAudioTracks()) {
        track.enabled = false;
      }
    }
  }

  gw.send({
    type: "VoiceStateSet",
    data: { self_mute: get(voiceMuted), self_deaf: deafened },
  });
}

export function cleanupVoice() {
  if (get(voiceConnected)) {
    leaveVoice();
  }
  gw = null;
  myUserId = null;
  cryptoKey = null;
}

// --- Gateway event handlers ---

export function handleVoiceStateUpdate(event: any) {
  const data = event.data;
  const userId: string = data.user_id;
  const sessionId: string | null = data.session_id ?? null;

  if (sessionId === null) {
    // User left voice
    voiceParticipants.update((m) => {
      m.delete(userId);
      return new Map(m);
    });
    cleanupPlayback(userId);
    return;
  }

  // User joined or updated
  voiceParticipants.update((m) => {
    const existing = m.get(userId);
    m.set(userId, {
      userId,
      username: data.username,
      sessionId,
      selfMute: data.self_mute,
      selfDeaf: data.self_deaf,
      speaking: existing?.speaking ?? false,
    });
    return new Map(m);
  });
}

export function handleVoiceSignal(_event: any) {
  // No longer used — audio is server-relayed, not P2P
}

export function handleVoiceAudioData(event: any) {
  if (!get(voiceConnected) || get(voiceDeafened)) return;

  const data = event.data;
  const fromUserId: string = data.from_user_id;
  const encryptedB64: string = data.data;

  // E2E decrypt then play
  decryptAudio(encryptedB64).then((pcmBytes) => {
    if (pcmBytes) {
      playReceivedAudio(fromUserId, pcmBytes);
    }
  });
}

// --- Audio capture (mic → server) ---

function setupAudioCapture(stream: MediaStream) {
  try {
    audioContext = new AudioContext();
    const source = audioContext.createMediaStreamSource(stream);

    // ScriptProcessorNode: buffer of 4096 samples, mono
    captureNode = audioContext.createScriptProcessor(4096, 1, 1);

    captureNode.onaudioprocess = (e) => {
      if (!gw || !get(voiceConnected) || get(voiceMuted)) return;

      const inputData = e.inputBuffer.getChannelData(0);

      // Downsample from native rate (usually 48kHz) to ~8kHz
      const sampleRate = audioContext!.sampleRate;
      const targetRate = 8000;
      const ratio = Math.round(sampleRate / targetRate);
      const downsampled = new Int16Array(Math.floor(inputData.length / ratio));

      for (let i = 0; i < downsampled.length; i++) {
        const s = Math.max(-1, Math.min(1, inputData[i * ratio]));
        downsampled[i] = s < 0 ? s * 0x8000 : s * 0x7FFF;
      }

      // E2E encrypt then send
      const pcmBytes = new Uint8Array(downsampled.buffer);
      encryptAudio(pcmBytes).then((encrypted) => {
        if (encrypted && gw) {
          gw.send({ type: "VoiceData", data: { data: encrypted } });
        }
      });
    };

    source.connect(captureNode);
    captureNode.connect(audioContext.destination); // Required for processing to work
  } catch (e) {
    console.error("[voice] Failed to setup audio capture:", e);
  }
}

// --- Audio playback (server → speaker) ---

function playReceivedAudio(userId: string, pcmBytes: Uint8Array) {
  try {
    const samples = new Int16Array(pcmBytes.buffer, pcmBytes.byteOffset, pcmBytes.byteLength / 2);

    // Get or create playback context for this user
    let pb = playbackContexts.get(userId);
    if (!pb) {
      const ctx = new AudioContext({ sampleRate: 8000 });
      pb = { ctx, nextPlayTime: ctx.currentTime };
      playbackContexts.set(userId, pb);
    }

    // Convert Int16 to Float32
    const float32 = new Float32Array(samples.length);
    for (let i = 0; i < samples.length; i++) {
      float32[i] = samples[i] / (samples[i] < 0 ? 0x8000 : 0x7FFF);
    }

    // Create audio buffer and schedule playback
    const buffer = pb.ctx.createBuffer(1, float32.length, 8000);
    buffer.getChannelData(0).set(float32);

    const source = pb.ctx.createBufferSource();
    source.buffer = buffer;
    source.connect(pb.ctx.destination);

    // Schedule seamless playback
    const now = pb.ctx.currentTime;
    if (pb.nextPlayTime < now) {
      pb.nextPlayTime = now + 0.01; // Small buffer to avoid clicks
    }
    source.start(pb.nextPlayTime);
    pb.nextPlayTime += buffer.duration;

    // Update speaking state for remote user
    const avg = float32.reduce((sum, v) => sum + Math.abs(v), 0) / float32.length;
    const speaking = avg > 0.01;
    voiceParticipants.update((m) => {
      const p = m.get(userId);
      if (p && p.speaking !== speaking) {
        m.set(userId, { ...p, speaking });
        return new Map(m);
      }
      return m;
    });
  } catch (e) {
    console.error("[voice] playback error:", e);
  }
}

function cleanupPlayback(userId: string) {
  const pb = playbackContexts.get(userId);
  if (pb) {
    pb.ctx.close().catch(() => {});
    playbackContexts.delete(userId);
  }
}

function cleanupAllPlayback() {
  for (const [userId] of playbackContexts) {
    cleanupPlayback(userId);
  }
}

// --- Speaking detection (VAD) for local mic ---

function setupVAD(stream: MediaStream) {
  if (vadInterval) clearInterval(vadInterval);

  try {
    const ctx = new AudioContext();
    const source = ctx.createMediaStreamSource(stream);
    const analyser = ctx.createAnalyser();
    analyser.fftSize = 512;
    analyser.smoothingTimeConstant = 0.4;
    source.connect(analyser);

    const data = new Uint8Array(analyser.frequencyBinCount);

    vadInterval = window.setInterval(() => {
      analyser.getByteFrequencyData(data);
      let sum = 0;
      for (let i = 0; i < data.length; i++) {
        sum += data[i];
      }
      const average = sum / data.length;
      const speaking = average > 15;
      localSpeaking.set(speaking);

      // Update our own participant speaking state
      if (myUserId) {
        voiceParticipants.update((m) => {
          const p = m.get(myUserId!);
          if (p && p.speaking !== speaking) {
            m.set(myUserId!, { ...p, speaking });
            return new Map(m);
          }
          return m;
        });
      }
    }, 100);
  } catch (e) {
    console.error("Failed to setup VAD:", e);
  }
}

// --- Cleanup helpers ---

function cleanupLocalMedia() {
  if (vadInterval) {
    clearInterval(vadInterval);
    vadInterval = null;
  }

  if (captureNode) {
    captureNode.disconnect();
    captureNode = null;
  }

  if (audioContext) {
    audioContext.close().catch(() => {});
    audioContext = null;
  }

  if (localStream) {
    for (const track of localStream.getTracks()) {
      track.stop();
    }
    localStream = null;
  }

  localSpeaking.set(false);
}
