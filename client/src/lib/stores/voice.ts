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
  videoStream: MediaStream | null;
  screenStream: MediaStream | null;
}

// --- Constants ---

const VOICE_CHANNEL_ID = "00000000-0000-0000-0000-000000000002";

// Audio quality: 16 kHz gives clear voice reproduction (telephone = 8 kHz,
// wideband VoIP = 16 kHz, fullband = 48 kHz). 16 kHz is the sweet spot for
// server-relayed voice: 2x quality of 8 kHz at manageable bandwidth.
const VOICE_SAMPLE_RATE = 16000;

// Capture buffer: 320 samples at 16 kHz = 20 ms frames (Opus standard frame size).
// Lower latency than the previous 4096-sample buffer at 48 kHz.
const CAPTURE_BUFFER_SAMPLES = 320;

// Jitter buffer: extra lead time (ms) to absorb network jitter in playback scheduling.
const JITTER_BUFFER_MS = 40;

// --- Stores ---

export const voiceConnected = writable(false);
export const voiceMuted = writable(false);
export const voiceDeafened = writable(false);
export const voiceParticipants = writable<Map<string, VoiceParticipant>>(new Map());
export const localSpeaking = writable(false);

export const voiceError = writable<string | null>(null);

// Video/screenshare stores
export const videoEnabled = writable(false);
export const screenShareEnabled = writable(false);
export const localVideoStream = writable<MediaStream | null>(null);
export const localScreenStream = writable<MediaStream | null>(null);

export const voiceParticipantList = derived(voiceParticipants, ($p) =>
  Array.from($p.values())
);

// --- Module state ---

let gw: Gateway | null = null;
let myUserId: string | null = null;
let localStream: MediaStream | null = null;
let audioContext: AudioContext | null = null;
let captureNode: AudioWorkletNode | null = null;
let vadInterval: number | null = null;
let vadContext: AudioContext | null = null;

// E2E encryption key (derived from channel key)
let cryptoKey: CryptoKey | null = null;

// WebRTC peer connections for video/screenshare (userId -> RTCPeerConnection)
const peerConnections: Map<string, RTCPeerConnection> = new Map();

// Track what we're sending to each peer so we can replace tracks
const videoSenders: Map<string, RTCRtpSender> = new Map();
const screenSenders: Map<string, RTCRtpSender> = new Map();

// Track whether we're the polite peer (for perfect negotiation pattern)
const politeFlags: Map<string, boolean> = new Map();

// Whether we're currently making an offer (per-peer, for perfect negotiation glare handling)
const makingOfferMap: Map<string, boolean> = new Map();

// Screen share stream IDs signaled by remote peers (userId -> set of screen stream IDs)
const peerScreenStreamIds: Map<string, Set<string>> = new Map();

// Buffered tracks that arrived before the participant was registered in the store
const pendingTracks: Map<string, { stream: MediaStream; isScreen: boolean; track: MediaStreamTrack }[]> = new Map();

// Playback state per remote user
const playbackContexts: Map<string, {
  ctx: AudioContext;
  nextPlayTime: number;
}> = new Map();

// --- E2E Crypto helpers ---
//
// SECURITY LIMITATION: No forward secrecy on voice encryption.
//
// The current implementation reuses the static AES-GCM channel key for every
// voice session. If this long-lived key is compromised, ALL past and future
// voice sessions encrypted with it can be decrypted.
//
// Ideal fix: Ephemeral per-session ECDH key exchange.
//   1. On VoiceJoin, each participant generates an ephemeral ECDH key pair.
//   2. Public keys are exchanged via the signaling channel (VoiceSignalSend).
//   3. Each pair of participants derives a shared secret via ECDH.
//   4. The shared secret is fed into HKDF (with session-specific salt/info)
//      to derive a unique AES-256-GCM session key.
//   5. For group calls, use a group key agreement (e.g., TreeKEM from MLS)
//      or a star topology where each pair has an independent session key.
//   6. Ephemeral keys are discarded when the voice session ends, providing
//      forward secrecy: compromise of the long-lived channel key does not
//      reveal past voice session content.
//
// This is an architectural change that requires a key exchange protocol and
// cannot be done as a simple patch. Tracked for future implementation.

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
    // Optimized audio constraints for high-quality voice chat:
    // - echoCancellation: prevents echo from speakers feeding back into mic
    // - noiseSuppression: reduces background noise (fans, keyboard, etc.)
    // - autoGainControl: normalizes volume levels across different mics
    // - sampleRate: 48kHz capture, we downsample to 16kHz before sending
    // - channelCount: mono is sufficient for voice and halves bandwidth
    localStream = await navigator.mediaDevices.getUserMedia({
      audio: {
        echoCancellation: true,
        noiseSuppression: true,
        autoGainControl: true,
        sampleRate: 48000,
        channelCount: 1,
      },
    });
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
  await setupAudioCapture(localStream);
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
    closePeerConnection(userId);
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
      videoStream: existing?.videoStream ?? null,
      screenStream: existing?.screenStream ?? null,
    });
    return new Map(m);
  });

  // If a new remote user joined and we're in the channel, set up a peer connection
  if (userId !== myUserId && get(voiceConnected)) {
    ensurePeerConnection(userId);
  }

  // Replay any tracks that arrived before this participant was registered
  const buffered = pendingTracks.get(userId);
  if (buffered && buffered.length > 0) {
    pendingTracks.delete(userId);
    voiceParticipants.update((m) => {
      const p = m.get(userId);
      if (p) {
        let updated = { ...p };
        for (const { stream, isScreen, track } of buffered) {
          if (isScreen) {
            updated.screenStream = stream;
          } else {
            updated.videoStream = stream;
          }
          // Re-attach the onended handler
          track.onended = () => {
            voiceParticipants.update((m2) => {
              const p2 = m2.get(userId);
              if (p2) {
                if (isScreen) {
                  m2.set(userId, { ...p2, screenStream: null });
                } else {
                  m2.set(userId, { ...p2, videoStream: null });
                }
                return new Map(m2);
              }
              return m2;
            });
          };
        }
        m.set(userId, updated);
        return new Map(m);
      }
      return m;
    });
  }
}

export async function handleVoiceSignal(event: any) {
  // Used for WebRTC P2P video/screenshare signaling.
  // Audio remains server-relayed, but video uses P2P connections.
  const data = event.data;
  const fromUserId: string = data.from_user_id;
  const signal = data.signal;

  if (!get(voiceConnected) || !gw || !myUserId) return;

  // Handle track metadata signals (camera vs screen share classification)
  if (signal.signal_type === "TrackInfo") {
    const streamId: string = signal.stream_id;
    const trackType: string = signal.track_type;
    if (trackType === "screen" && streamId) {
      const ids = peerScreenStreamIds.get(fromUserId) || new Set();
      ids.add(streamId);
      peerScreenStreamIds.set(fromUserId, ids);
    } else if (trackType === "camera_removed") {
      // Peer stopped screen share — clear their screen stream IDs
      peerScreenStreamIds.delete(fromUserId);
    }
    return;
  }

  const pc = ensurePeerConnection(fromUserId);

  try {
    if (signal.signal_type === "Offer") {
      // Perfect negotiation: handle incoming offer (per-peer makingOffer flag)
      const offerCollision = (makingOfferMap.get(fromUserId) ?? false) || pc.signalingState !== "stable";
      const isPolite = politeFlags.get(fromUserId) ?? false;
      const ignoreOffer = !isPolite && offerCollision;

      if (ignoreOffer) return;

      await pc.setRemoteDescription(new RTCSessionDescription({ type: "offer", sdp: signal.sdp }));
      await pc.setLocalDescription();

      if (pc.localDescription) {
        gw.send({
          type: "VoiceSignalSend",
          data: {
            target_user_id: fromUserId,
            signal: {
              signal_type: "Answer",
              sdp: pc.localDescription.sdp,
            },
          },
        });
      }
    } else if (signal.signal_type === "Answer") {
      await pc.setRemoteDescription(new RTCSessionDescription({ type: "answer", sdp: signal.sdp }));
    } else if (signal.signal_type === "IceCandidate") {
      if (signal.candidate) {
        await pc.addIceCandidate(new RTCIceCandidate({
          candidate: signal.candidate,
          sdpMid: signal.sdp_mid ?? undefined,
          sdpMLineIndex: signal.sdp_m_line_index ?? undefined,
        }));
      }
    }
  } catch (e) {
    console.error("[voice] Failed to handle signal from", fromUserId, e);
  }
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

async function setupAudioCapture(stream: MediaStream) {
  try {
    audioContext = new AudioContext({ sampleRate: 48000 });
    const source = audioContext.createMediaStreamSource(stream);

    // Load the AudioWorklet processor module (replaces deprecated ScriptProcessorNode)
    await audioContext.audioWorklet.addModule("/capture-processor.js");

    captureNode = new AudioWorkletNode(audioContext, "capture-processor", {
      channelCount: 1,
      numberOfInputs: 1,
      numberOfOutputs: 0,
    });

    captureNode.port.onmessage = (e: MessageEvent) => {
      if (!gw || !get(voiceConnected) || get(voiceMuted)) return;

      const inputData: Float32Array = e.data.pcmData;

      // Downsample from native rate (48kHz) to 16kHz (wideband voice).
      // Linear interpolation for smoother downsampling than nearest-neighbor.
      const sampleRate = audioContext!.sampleRate;
      const targetRate = VOICE_SAMPLE_RATE;
      const ratio = sampleRate / targetRate;
      const outputLen = Math.floor(inputData.length / ratio);
      const downsampled = new Int16Array(outputLen);

      for (let i = 0; i < outputLen; i++) {
        const srcIdx = i * ratio;
        const idx0 = Math.floor(srcIdx);
        const idx1 = Math.min(idx0 + 1, inputData.length - 1);
        const frac = srcIdx - idx0;
        const sample = inputData[idx0] * (1 - frac) + inputData[idx1] * frac;
        const clamped = Math.max(-1, Math.min(1, sample));
        downsampled[i] = clamped < 0 ? clamped * 0x8000 : clamped * 0x7FFF;
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
  } catch (e) {
    console.error("[voice] Failed to setup audio capture:", e);
  }
}

// --- Audio playback (server → speaker) ---

function playReceivedAudio(userId: string, pcmBytes: Uint8Array) {
  try {
    const samples = new Int16Array(pcmBytes.buffer, pcmBytes.byteOffset, pcmBytes.byteLength / 2);

    // Get or create playback context for this user (16 kHz wideband)
    let pb = playbackContexts.get(userId);
    if (!pb) {
      const ctx = new AudioContext({ sampleRate: VOICE_SAMPLE_RATE });
      pb = { ctx, nextPlayTime: ctx.currentTime };
      playbackContexts.set(userId, pb);
    }

    // Convert Int16 to Float32
    const float32 = new Float32Array(samples.length);
    for (let i = 0; i < samples.length; i++) {
      float32[i] = samples[i] / (samples[i] < 0 ? 0x8000 : 0x7FFF);
    }

    // Create audio buffer and schedule playback
    const buffer = pb.ctx.createBuffer(1, float32.length, VOICE_SAMPLE_RATE);
    buffer.getChannelData(0).set(float32);

    const source = pb.ctx.createBufferSource();
    source.buffer = buffer;
    source.connect(pb.ctx.destination);

    // Jitter buffer: schedule playback with lead time to absorb network jitter.
    // If we've fallen behind real-time (packets arrived late / gap in audio),
    // reset the schedule with the jitter buffer offset.
    const now = pb.ctx.currentTime;
    const jitterOffset = JITTER_BUFFER_MS / 1000;
    if (pb.nextPlayTime < now) {
      pb.nextPlayTime = now + jitterOffset;
    }
    source.start(pb.nextPlayTime);
    pb.nextPlayTime += buffer.duration;

    // RMS-based speaking detection (more accurate than simple average)
    let sumSquares = 0;
    for (let i = 0; i < float32.length; i++) {
      sumSquares += float32[i] * float32[i];
    }
    const rms = Math.sqrt(sumSquares / float32.length);
    const speaking = rms > 0.01;
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
  if (vadContext) {
    vadContext.close().catch(() => {});
    vadContext = null;
  }

  try {
    const ctx = new AudioContext();
    vadContext = ctx;
    const source = ctx.createMediaStreamSource(stream);
    const analyser = ctx.createAnalyser();
    // Larger FFT for better frequency resolution; higher smoothing for stable readings
    analyser.fftSize = 1024;
    analyser.smoothingTimeConstant = 0.5;
    source.connect(analyser);

    const timeDomainData = new Float32Array(analyser.fftSize);
    // Hysteresis thresholds to prevent flickering
    const SPEAK_THRESHOLD = 0.015;  // RMS level to start speaking
    const SILENCE_THRESHOLD = 0.008; // RMS level to stop speaking
    let isSpeaking = false;

    vadInterval = window.setInterval(() => {
      analyser.getFloatTimeDomainData(timeDomainData);
      // RMS-based voice activity detection (more reliable than frequency averaging)
      let sumSquares = 0;
      for (let i = 0; i < timeDomainData.length; i++) {
        sumSquares += timeDomainData[i] * timeDomainData[i];
      }
      const rms = Math.sqrt(sumSquares / timeDomainData.length);

      // Hysteresis: different thresholds for start vs stop to prevent flickering
      if (isSpeaking) {
        if (rms < SILENCE_THRESHOLD) isSpeaking = false;
      } else {
        if (rms > SPEAK_THRESHOLD) isSpeaking = true;
      }

      localSpeaking.set(isSpeaking);

      // Update our own participant speaking state
      if (myUserId) {
        voiceParticipants.update((m) => {
          const p = m.get(myUserId!);
          if (p && p.speaking !== isSpeaking) {
            m.set(myUserId!, { ...p, speaking: isSpeaking });
            return new Map(m);
          }
          return m;
        });
      }
    }, 50); // 50ms polling for more responsive speaking indicator
  } catch (e) {
    console.error("Failed to setup VAD:", e);
  }
}

// --- WebRTC Peer Connection Management (Video/Screenshare) ---

const ICE_SERVERS: RTCIceServer[] = [
  { urls: "stun:stun.l.google.com:19302" },
  { urls: "stun:stun1.l.google.com:19302" },
];

function ensurePeerConnection(remoteUserId: string): RTCPeerConnection {
  let pc = peerConnections.get(remoteUserId);
  if (pc) return pc;

  pc = new RTCPeerConnection({ iceServers: ICE_SERVERS });
  peerConnections.set(remoteUserId, pc);

  // Perfect negotiation: determine polite/impolite peer based on user ID comparison
  // The peer with the lexicographically smaller ID is "polite"
  politeFlags.set(remoteUserId, myUserId! < remoteUserId);

  // ICE candidate handler
  pc.onicecandidate = (e) => {
    if (e.candidate && gw) {
      gw.send({
        type: "VoiceSignalSend",
        data: {
          target_user_id: remoteUserId,
          signal: {
            signal_type: "IceCandidate",
            candidate: e.candidate.candidate,
            sdp_mid: e.candidate.sdpMid ?? null,
            sdp_m_line_index: e.candidate.sdpMLineIndex ?? null,
          },
        },
      });
    }
  };

  // Handle negotiation needed (fires when tracks are added/removed)
  pc.onnegotiationneeded = async () => {
    if (!gw) return;
    try {
      makingOfferMap.set(remoteUserId, true);
      await pc!.setLocalDescription();
      if (pc!.localDescription) {
        gw.send({
          type: "VoiceSignalSend",
          data: {
            target_user_id: remoteUserId,
            signal: {
              signal_type: "Offer",
              sdp: pc!.localDescription.sdp,
            },
          },
        });
      }
    } catch (e) {
      console.error("[voice] Negotiation failed for", remoteUserId, e);
    } finally {
      makingOfferMap.set(remoteUserId, false);
    }
  };

  // Handle incoming tracks (video/screen from remote peer)
  pc.ontrack = (e) => {
    const stream = e.streams[0];
    if (!stream) return;

    // Determine if this is a screen share by checking if the peer signaled
    // this stream ID as a screen share via TrackInfo.
    const screenIds = peerScreenStreamIds.get(remoteUserId);
    const isScreen = screenIds?.has(stream.id) ?? false;

    voiceParticipants.update((m) => {
      const p = m.get(remoteUserId);
      if (p) {
        if (isScreen) {
          m.set(remoteUserId, { ...p, screenStream: stream });
        } else {
          m.set(remoteUserId, { ...p, videoStream: stream });
        }
        return new Map(m);
      }
      // Participant not registered yet — buffer the track for replay
      const buf = pendingTracks.get(remoteUserId) || [];
      buf.push({ stream, isScreen, track: e.track });
      pendingTracks.set(remoteUserId, buf);
      return m;
    });

    // When the track ends, clear the stream reference
    e.track.onended = () => {
      voiceParticipants.update((m) => {
        const p = m.get(remoteUserId);
        if (p) {
          if (isScreen) {
            m.set(remoteUserId, { ...p, screenStream: null });
          } else {
            m.set(remoteUserId, { ...p, videoStream: null });
          }
          return new Map(m);
        }
        return m;
      });
    };
  };

  pc.onconnectionstatechange = () => {
    const state = pc!.connectionState;
    if (state === "failed") {
      console.warn("[voice] Peer connection to", remoteUserId, "failed, attempting ICE restart");
      // restartIce() triggers onnegotiationneeded which creates a new offer with ICE restart
      pc!.restartIce();
    } else if (state === "disconnected") {
      console.warn("[voice] Peer connection to", remoteUserId, "disconnected");
    }
  };

  // If we have local video or screen share active, add tracks to this new connection
  const currentVideoStream = get(localVideoStream);
  if (currentVideoStream) {
    for (const track of currentVideoStream.getVideoTracks()) {
      const sender = pc.addTrack(track, currentVideoStream);
      videoSenders.set(remoteUserId, sender);
    }
  }

  const currentScreenStream = get(localScreenStream);
  if (currentScreenStream) {
    // Signal screen stream ID before adding tracks so the receiver classifies them correctly
    if (gw) {
      gw.send({
        type: "VoiceSignalSend",
        data: {
          target_user_id: remoteUserId,
          signal: {
            signal_type: "TrackInfo",
            track_type: "screen",
            stream_id: currentScreenStream.id,
          },
        },
      });
    }
    for (const track of currentScreenStream.getVideoTracks()) {
      const sender = pc.addTrack(track, currentScreenStream);
      screenSenders.set(remoteUserId, sender);
    }
  }

  return pc;
}

function closePeerConnection(remoteUserId: string) {
  const pc = peerConnections.get(remoteUserId);
  if (pc) {
    pc.close();
    peerConnections.delete(remoteUserId);
  }
  videoSenders.delete(remoteUserId);
  screenSenders.delete(remoteUserId);
  politeFlags.delete(remoteUserId);
  makingOfferMap.delete(remoteUserId);
  peerScreenStreamIds.delete(remoteUserId);
  pendingTracks.delete(remoteUserId);
}

function closeAllPeerConnections() {
  for (const [userId] of peerConnections) {
    closePeerConnection(userId);
  }
}

// --- Video toggle ---

export async function toggleCamera() {
  if (!get(voiceConnected)) return;

  if (get(videoEnabled)) {
    // Turn off camera
    const stream = get(localVideoStream);
    if (stream) {
      for (const track of stream.getTracks()) {
        track.stop();
      }
      // Remove video tracks from all peer connections
      for (const [userId, sender] of videoSenders) {
        const pc = peerConnections.get(userId);
        if (pc) {
          pc.removeTrack(sender);
        }
      }
      videoSenders.clear();
    }
    localVideoStream.set(null);
    videoEnabled.set(false);
  } else {
    // Turn on camera
    try {
      const stream = await navigator.mediaDevices.getUserMedia({
        video: {
          width: { ideal: 640, max: 1280 },
          height: { ideal: 480, max: 720 },
          frameRate: { ideal: 24, max: 30 },
        },
      });
      localVideoStream.set(stream);
      videoEnabled.set(true);

      // Add video track to all existing peer connections
      for (const [userId, pc] of peerConnections) {
        for (const track of stream.getVideoTracks()) {
          const sender = pc.addTrack(track, stream);
          videoSenders.set(userId, sender);
        }
      }
    } catch (e: any) {
      console.error("[voice] Camera access failed:", e);
      voiceError.set("Camera access denied: " + (e.message || e.name));
    }
  }
}

// --- Screen share toggle ---

export async function toggleScreenShare() {
  if (!get(voiceConnected)) return;

  if (get(screenShareEnabled)) {
    // Stop screen share
    const stream = get(localScreenStream);
    if (stream) {
      for (const track of stream.getTracks()) {
        track.stop();
      }
      // Remove screen tracks from all peer connections
      for (const [userId, sender] of screenSenders) {
        const pc = peerConnections.get(userId);
        if (pc) {
          pc.removeTrack(sender);
        }
        // Signal to peer that screen share stopped
        if (gw) {
          gw.send({
            type: "VoiceSignalSend",
            data: {
              target_user_id: userId,
              signal: {
                signal_type: "TrackInfo",
                track_type: "camera_removed",
                stream_id: stream.id,
              },
            },
          });
        }
      }
      screenSenders.clear();
    }
    localScreenStream.set(null);
    screenShareEnabled.set(false);
  } else {
    // Start screen share
    try {
      // Use a stream ID prefix to distinguish screen share from camera
      const stream = await navigator.mediaDevices.getDisplayMedia({
        video: {
          width: { ideal: 1920, max: 1920 },
          height: { ideal: 1080, max: 1080 },
          frameRate: { ideal: 15, max: 30 },
        },
        audio: false,
      });

      localScreenStream.set(stream);
      screenShareEnabled.set(true);

      // Signal to all peers that this stream is a screen share BEFORE adding tracks.
      // The signal arrives before the SDP offer, so the receiver knows to classify it.
      for (const [userId] of peerConnections) {
        if (gw) {
          gw.send({
            type: "VoiceSignalSend",
            data: {
              target_user_id: userId,
              signal: {
                signal_type: "TrackInfo",
                track_type: "screen",
                stream_id: stream.id,
              },
            },
          });
        }
      }

      // Add screen track to all existing peer connections
      for (const [userId, pc] of peerConnections) {
        for (const track of stream.getVideoTracks()) {
          const sender = pc.addTrack(track, stream);
          screenSenders.set(userId, sender);
        }
      }

      // Handle user stopping the share via the browser's "Stop Sharing" button
      stream.getVideoTracks()[0].onended = () => {
        // Clean up screen share state
        for (const [userId, sender] of screenSenders) {
          const pc = peerConnections.get(userId);
          if (pc) {
            pc.removeTrack(sender);
          }
        }
        screenSenders.clear();
        localScreenStream.set(null);
        screenShareEnabled.set(false);
      };
    } catch (e: any) {
      console.error("[voice] Screen share failed:", e);
      // User probably cancelled the picker — don't show error for that
      if (e.name !== "NotAllowedError") {
        voiceError.set("Screen share failed: " + (e.message || e.name));
      }
    }
  }
}

// --- Cleanup helpers ---

function cleanupLocalMedia() {
  if (vadInterval) {
    clearInterval(vadInterval);
    vadInterval = null;
  }

  if (vadContext) {
    vadContext.close().catch(() => {});
    vadContext = null;
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

  // Clean up video stream
  const vidStream = get(localVideoStream);
  if (vidStream) {
    for (const track of vidStream.getTracks()) {
      track.stop();
    }
    localVideoStream.set(null);
  }
  videoEnabled.set(false);

  // Clean up screen share stream
  const scrStream = get(localScreenStream);
  if (scrStream) {
    for (const track of scrStream.getTracks()) {
      track.stop();
    }
    localScreenStream.set(null);
  }
  screenShareEnabled.set(false);

  // Clean up all peer connections
  closeAllPeerConnections();

  localSpeaking.set(false);
}
