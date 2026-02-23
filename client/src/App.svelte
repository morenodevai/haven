<script lang="ts">
  import { auth } from "./lib/stores/auth";
  import { channelKey, loadMessages, handleIncomingMessage, handleReactionAdd, handleReactionRemove } from "./lib/stores/messages";
  import { initVoice, handleVoiceStateUpdate, handleVoiceSignal, handleVoiceAudioData, cleanupVoice } from "./lib/stores/voice";
  import { checkForUpdate, updateAvailable, updateVersion, updateProgress, updateError, installUpdate } from "./lib/stores/updater";
  import { activeChannel } from "./lib/stores/channels";
  import { handlePresenceUpdate } from "./lib/stores/presence";
  import { initTransfers, cleanupTransfers, handleFileOffer, handleFileAccept, handleFileReject, handleFileSignal, handleFileChunk, handleFileDone, handleFileAck, handleBinaryMessage } from "./lib/stores/transfers";
  import { Gateway } from "./lib/ipc/gateway";
  import { getBaseUrl } from "./lib/ipc/api";
  import { invoke } from "@tauri-apps/api/core";
  import Login from "./lib/components/auth/Login.svelte";
  import Sidebar from "./lib/components/sidebar/Sidebar.svelte";
  import MessageList from "./lib/components/chat/MessageList.svelte";
  import MessageInput from "./lib/components/chat/MessageInput.svelte";
  import FileChannel from "./lib/components/chat/FileChannel.svelte";
  import VideoGrid from "./lib/components/voice/VideoGrid.svelte";
  import { voiceConnected, videoEnabled, screenShareEnabled, voiceParticipantList } from "./lib/stores/voice";
  import { onMount } from "svelte";

  // Derived: true when any video tiles should be visible
  let showVideoGrid = $derived(
    $voiceConnected && (
      $videoEnabled ||
      $screenShareEnabled ||
      $voiceParticipantList.some(p => p.videoStream !== null || p.screenStream !== null)
    )
  );

  let gateway: Gateway | null = $state(null);
  let connected = $state(false);
  let showUpdateToast = $state(false);

  // Check for updates on mount
  onMount(() => {
    checkForUpdate().then(() => {
      // Show toast briefly if update is available
      const unsub = updateAvailable.subscribe((available) => {
        if (available) showUpdateToast = true;
      });
      return unsub;
    });
  });

  // Connect to gateway when logged in + have channel key
  $effect(() => {
    if ($auth.loggedIn && $auth.token && $channelKey) {
      // Load message history
      loadMessages();

      // Connect WebSocket
      const wsUrl = getBaseUrl().replace("http", "ws") + "/gateway?token=" + encodeURIComponent($auth.token!);
      const gw = new Gateway(wsUrl, () => $auth.token!);

      gw.on("Ready", () => {
        connected = true;
        // Subscribe to all channels so the server delivers scoped events
        gw.send({
          type: "Subscribe",
          data: {
            channel_ids: [
              "00000000-0000-0000-0000-000000000001", // general
              "00000000-0000-0000-0000-000000000002", // voice
              "00000000-0000-0000-0000-000000000003", // file-sharing
            ],
          },
        });
        // Resync messages after reconnect to catch anything missed while offline
        loadMessages();
      });

      gw.on("MessageCreate", (event) => {
        handleIncomingMessage(event);
      });

      gw.on("ReactionAdd", (event) => {
        handleReactionAdd(event);
      });

      gw.on("ReactionRemove", (event) => {
        handleReactionRemove(event);
      });

      gw.on("VoiceStateUpdate", (event) => {
        handleVoiceStateUpdate(event);
      });

      gw.on("VoiceSignal", (event) => {
        handleVoiceSignal(event);
      });

      gw.on("VoiceAudioData", (event) => {
        handleVoiceAudioData(event);
      });

      // File transfer signaling events
      gw.on("FileOffer", (event) => handleFileOffer(event));
      gw.on("FileAccept", (event) => handleFileAccept(event));
      gw.on("FileReject", (event) => handleFileReject(event));
      gw.on("FileSignal", (event) => handleFileSignal(event));

      // Server-relayed file chunks (fallback when P2P fails)
      gw.on("FileChunk", (event) => handleFileChunk(event));
      gw.on("FileDone", (event) => handleFileDone(event));
      gw.on("FileAck", (event) => handleFileAck(event));

      // Binary WebSocket frames (file transfer fast path — no base64, no JSON)
      gw.on("__binary__", (data) => handleBinaryMessage(data));

      // Presence tracking for online users
      gw.on("PresenceUpdate", (event) => handlePresenceUpdate(event));

      gw.on("Disconnected", () => {
        connected = false;
      });

      gw.connect();
      gateway = gw;
      initVoice(gw, $auth.userId!);
      initTransfers(gw);

      // Connect native TCP relay for high-speed file transfers
      if ((window as any).__TAURI_INTERNALS__) {
        try {
          const baseUrl = new URL(getBaseUrl());
          const serverHost = baseUrl.hostname;
          const httpPort = parseInt(baseUrl.port) || 3000;
          const relayPort = httpPort + 1;
          invoke("transfer_connect", {
            serverHost,
            relayPort,
            jwtToken: $auth.token!,
          }).then(() => {
            console.log("[App] TCP relay connected on port", relayPort);
          }).catch((e: any) => {
            console.warn("[App] TCP relay connect failed (using WS fallback):", e);
          });
        } catch (e) {
          console.warn("[App] TCP relay setup error:", e);
        }
      }

      return () => {
        cleanupTransfers();
        cleanupVoice();
        gw.disconnect();
        gateway = null;
        connected = false;
      };
    }
  });
</script>

{#if !$auth.loggedIn || !$channelKey}
  <Login />
{:else}
  <div class="app-layout">
    <Sidebar />
    <div class="main-content">
      <div class="channel-header">
        <span class="hash">#</span>
        <span class="channel-name">{$activeChannel === "general" ? "general" : "file-sharing"}</span>
        {#if $updateAvailable}
          <button
            class="update-btn"
            onclick={() => installUpdate()}
            disabled={$updateProgress === "downloading" || $updateProgress === "installing"}
            title="Update to {$updateVersion}"
          >
            {#if $updateProgress === "downloading"}
              <svg class="spin" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M21 12a9 9 0 11-6.219-8.56"/></svg>
            {:else if $updateProgress === "error"}
              <span style="color: var(--error); font-size: 11px;" title={$updateError}>Failed: {$updateError.slice(0, 60)}</span>
            {:else}
              <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <path d="M21 15v4a2 2 0 01-2 2H5a2 2 0 01-2-2v-4"/>
                <polyline points="7 10 12 15 17 10"/>
                <line x1="12" y1="15" x2="12" y2="3"/>
              </svg>
            {/if}
          </button>
        {/if}
        <div class="status" class:online={connected}>
          {connected ? "Connected" : "Connecting..."}
        </div>
      </div>
      {#if showVideoGrid}
        <VideoGrid />
      {/if}
      {#if $activeChannel === "general"}
        <MessageList />
        <MessageInput />
      {:else if $activeChannel === "file-sharing"}
        <FileChannel />
      {/if}
    </div>
  </div>
{/if}

{#if showUpdateToast}
  <div class="update-toast">
    <span>Haven {$updateVersion} is available</span>
    <button class="toast-action" onclick={() => { installUpdate(); showUpdateToast = false; }}>Update</button>
    <button class="toast-dismiss" onclick={() => showUpdateToast = false}>✕</button>
  </div>
{/if}

<style>
  .app-layout {
    display: flex;
    height: 100%;
  }

  .main-content {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  .channel-header {
    padding: 12px 16px;
    border-bottom: 1px solid var(--border);
    display: flex;
    align-items: center;
    gap: 6px;
    flex-shrink: 0;
  }

  .hash {
    color: var(--text-muted);
    font-size: 20px;
    font-weight: 600;
  }

  .channel-name {
    font-weight: 700;
    font-size: 16px;
  }

  .status {
    margin-left: auto;
    font-size: 12px;
    color: var(--text-muted);
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .status::before {
    content: "";
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--error);
  }

  .status.online::before {
    background: var(--success);
  }

  .update-btn {
    background: none;
    border: 1px solid var(--accent);
    color: var(--accent);
    border-radius: 6px;
    padding: 4px 8px;
    cursor: pointer;
    display: flex;
    align-items: center;
    gap: 4px;
    font-size: 12px;
    transition: background 0.15s;
  }

  .update-btn:hover:not(:disabled) {
    background: rgba(88, 101, 242, 0.1);
  }

  .update-btn:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .spin {
    animation: spin 1s linear infinite;
  }

  @keyframes spin {
    from { transform: rotate(0deg); }
    to { transform: rotate(360deg); }
  }

  .update-toast {
    position: fixed;
    bottom: 20px;
    right: 20px;
    background: var(--bg-secondary, #1e1f22);
    border: 1px solid var(--accent);
    border-radius: 10px;
    padding: 10px 14px;
    display: flex;
    align-items: center;
    gap: 10px;
    font-size: 13px;
    color: var(--text-primary);
    z-index: 1000;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.3);
    animation: slideIn 0.3s ease;
  }

  @keyframes slideIn {
    from { opacity: 0; transform: translateY(10px); }
    to { opacity: 1; transform: translateY(0); }
  }

  .toast-action {
    background: var(--accent);
    border: none;
    color: white;
    border-radius: 6px;
    padding: 4px 12px;
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
  }

  .toast-action:hover {
    background: var(--accent-hover, #4752c4);
  }

  .toast-dismiss {
    background: none;
    border: none;
    color: var(--text-muted);
    cursor: pointer;
    font-size: 14px;
    padding: 2px;
    line-height: 1;
  }

  .toast-dismiss:hover {
    color: var(--text-primary);
  }
</style>
