<script lang="ts">
  import {
    voiceConnected,
    voiceParticipantList,
    localVideoStream,
    localScreenStream,
    videoEnabled,
    screenShareEnabled,
  } from "../../stores/voice";
  import { auth } from "../../stores/auth";

  // Determine if any video is active (local or remote)
  let hasAnyVideo = $derived(
    $videoEnabled ||
    $screenShareEnabled ||
    $voiceParticipantList.some(
      (p) => p.videoStream !== null || p.screenStream !== null
    )
  );

  // Build the list of video tiles to render
  interface VideoTile {
    id: string;
    label: string;
    stream: MediaStream;
    isLocal: boolean;
    isScreen: boolean;
    muted: boolean;
  }

  let tiles = $derived.by(() => {
    const result: VideoTile[] = [];

    // Local camera
    if ($localVideoStream) {
      result.push({
        id: "local-camera",
        label: $auth.username ?? "You",
        stream: $localVideoStream,
        isLocal: true,
        isScreen: false,
        muted: false,
      });
    }

    // Local screen share
    if ($localScreenStream) {
      result.push({
        id: "local-screen",
        label: ($auth.username ?? "You") + " (Screen)",
        stream: $localScreenStream,
        isLocal: true,
        isScreen: true,
        muted: false,
      });
    }

    // Remote participants' video and screen streams
    for (const p of $voiceParticipantList) {
      if (p.userId === $auth.userId) continue;

      if (p.videoStream) {
        result.push({
          id: `${p.userId}-camera`,
          label: p.username,
          stream: p.videoStream,
          isLocal: false,
          isScreen: false,
          muted: p.selfMute,
        });
      }

      if (p.screenStream) {
        result.push({
          id: `${p.userId}-screen`,
          label: p.username + " (Screen)",
          stream: p.screenStream,
          isLocal: false,
          isScreen: true,
          muted: p.selfMute,
        });
      }
    }

    return result;
  });

  // Compute grid columns based on tile count
  let gridCols = $derived(
    tiles.length <= 1 ? 1 : tiles.length <= 4 ? 2 : tiles.length <= 9 ? 3 : 4
  );

  // Bind video element to stream using an action
  function streamAction(node: HTMLVideoElement, stream: MediaStream) {
    node.srcObject = stream;

    return {
      update(newStream: MediaStream) {
        if (node.srcObject !== newStream) {
          node.srcObject = newStream;
        }
      },
      destroy() {
        node.srcObject = null;
      },
    };
  }
</script>

{#if $voiceConnected && hasAnyVideo}
  <div class="video-grid" style="--cols: {gridCols}">
    {#each tiles as tile (tile.id)}
      <div class="video-tile" class:screen-share={tile.isScreen}>
        <video
          autoplay
          playsinline
          muted={tile.isLocal}
          use:streamAction={tile.stream}
          class:mirror={tile.isLocal && !tile.isScreen}
        ></video>
        <div class="tile-label">
          <span class="tile-name">{tile.label}</span>
          {#if tile.muted && !tile.isLocal}
            <svg class="mute-icon" width="12" height="12" viewBox="0 0 24 24" fill="currentColor">
              <path d="M19 11h-1.7c0 .74-.16 1.43-.43 2.05l1.23 1.23c.56-.98.9-2.09.9-3.28zm-4.02.17c0-.06.02-.11.02-.17V5c0-1.66-1.34-3-3-3S9 3.34 9 5v.18l5.98 5.99zM4.27 3L3 4.27l6.01 6.01V11c0 1.66 1.33 3 2.99 3 .22 0 .44-.03.65-.08l1.66 1.66c-.71.33-1.5.52-2.31.52-2.76 0-5.3-2.1-5.3-5.1H5c0 3.41 2.72 6.23 6 6.72V21h2v-3.28c.91-.13 1.77-.45 2.54-.9L19.73 21 21 19.73 4.27 3z"/>
            </svg>
          {/if}
        </div>
      </div>
    {/each}
  </div>
{/if}

<style>
  .video-grid {
    display: grid;
    grid-template-columns: repeat(var(--cols), 1fr);
    gap: 4px;
    padding: 8px;
    background: var(--bg-primary);
    flex: 1;
    min-height: 0;
    overflow: auto;
  }

  .video-tile {
    position: relative;
    background: #0a0a0f;
    border-radius: 8px;
    overflow: hidden;
    aspect-ratio: 16 / 9;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .video-tile.screen-share {
    grid-column: span 2;
    aspect-ratio: auto;
    max-height: 70vh;
  }

  video {
    width: 100%;
    height: 100%;
    object-fit: contain;
    border-radius: 8px;
  }

  video.mirror {
    transform: scaleX(-1);
  }

  .tile-label {
    position: absolute;
    bottom: 6px;
    left: 6px;
    display: flex;
    align-items: center;
    gap: 4px;
    background: rgba(0, 0, 0, 0.7);
    color: white;
    padding: 3px 8px;
    border-radius: 4px;
    font-size: 12px;
    font-weight: 600;
  }

  .mute-icon {
    color: var(--error, #ef4444);
    flex-shrink: 0;
  }
</style>
