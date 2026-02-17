<script lang="ts">
  import {
    voiceConnected,
    voiceMuted,
    voiceDeafened,
    voiceError,
    voiceParticipantList,
    joinVoice,
    leaveVoice,
    toggleMute,
    toggleDeafen,
  } from "../../stores/voice";
</script>

<div class="voice-section">
  <div class="voice-header">VOICE CHANNELS</div>

  {#if $voiceError}
    <div class="voice-error">{$voiceError}</div>
  {/if}

  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="voice-channel"
    class:active={$voiceConnected}
    onclick={() => { if (!$voiceConnected) joinVoice(); }}
  >
    <svg class="voice-icon" width="16" height="16" viewBox="0 0 24 24" fill="currentColor">
      <path d="M12 14c1.66 0 3-1.34 3-3V5c0-1.66-1.34-3-3-3S9 3.34 9 5v6c0 1.66 1.34 3 3 3z"/>
      <path d="M17 11c0 2.76-2.24 5-5 5s-5-2.24-5-5H5c0 3.53 2.61 6.43 6 6.92V21h2v-3.08c3.39-.49 6-3.39 6-6.92h-2z"/>
    </svg>
    Voice
  </div>

  {#if $voiceParticipantList.length > 0}
    <div class="participants">
      {#each $voiceParticipantList as participant (participant.userId)}
        <div class="participant">
          <div class="participant-avatar" class:speaking={participant.speaking && !participant.selfMute}>
            {participant.username.charAt(0).toUpperCase()}
          </div>
          <span class="participant-name">{participant.username}</span>
          {#if participant.selfMute}
            <svg class="status-icon muted" width="14" height="14" viewBox="0 0 24 24" fill="currentColor">
              <path d="M19 11h-1.7c0 .74-.16 1.43-.43 2.05l1.23 1.23c.56-.98.9-2.09.9-3.28zm-4.02.17c0-.06.02-.11.02-.17V5c0-1.66-1.34-3-3-3S9 3.34 9 5v.18l5.98 5.99zM4.27 3L3 4.27l6.01 6.01V11c0 1.66 1.33 3 2.99 3 .22 0 .44-.03.65-.08l1.66 1.66c-.71.33-1.5.52-2.31.52-2.76 0-5.3-2.1-5.3-5.1H5c0 3.41 2.72 6.23 6 6.72V21h2v-3.28c.91-.13 1.77-.45 2.54-.9L19.73 21 21 19.73 4.27 3z"/>
            </svg>
          {/if}
          {#if participant.selfDeaf}
            <svg class="status-icon deafened" width="14" height="14" viewBox="0 0 24 24" fill="currentColor">
              <path d="M3.63 3.63a.996.996 0 000 1.41L7.29 8.7 7 9H4c-.55 0-1 .45-1 1v4c0 .55.45 1 1 1h3l3.29 3.29c.63.63 1.71.18 1.71-.71v-4.17l4.18 4.18c-.49.37-1.02.68-1.6.91-.36.15-.58.53-.58.92 0 .72.73 1.18 1.39.91.8-.33 1.55-.77 2.22-1.31l1.34 1.34a.996.996 0 101.41-1.41L5.05 3.63c-.39-.39-1.02-.39-1.42 0zM19 12c0 .82-.15 1.61-.41 2.34l1.53 1.53c.56-1.17.88-2.48.88-3.87 0-3.83-2.4-7.11-5.78-8.4-.59-.23-1.22.23-1.22.86v.19c0 .38.25.71.61.85C17.18 6.54 19 9.06 19 12zm-8.71-6.29l-.17.17L12 7.76V6.41c0-.89-1.08-1.33-1.71-.7zM16.5 12A4.5 4.5 0 0014 7.97v1.79l2.48 2.48c.01-.08.02-.16.02-.24z"/>
            </svg>
          {/if}
        </div>
      {/each}
    </div>
  {/if}

  {#if $voiceConnected}
    <div class="voice-controls">
      <button
        class="control-btn"
        class:active={$voiceMuted}
        onclick={toggleMute}
        title={$voiceMuted ? "Unmute" : "Mute"}
      >
        {#if $voiceMuted}
          <svg width="18" height="18" viewBox="0 0 24 24" fill="currentColor">
            <path d="M19 11h-1.7c0 .74-.16 1.43-.43 2.05l1.23 1.23c.56-.98.9-2.09.9-3.28zm-4.02.17c0-.06.02-.11.02-.17V5c0-1.66-1.34-3-3-3S9 3.34 9 5v.18l5.98 5.99zM4.27 3L3 4.27l6.01 6.01V11c0 1.66 1.33 3 2.99 3 .22 0 .44-.03.65-.08l1.66 1.66c-.71.33-1.5.52-2.31.52-2.76 0-5.3-2.1-5.3-5.1H5c0 3.41 2.72 6.23 6 6.72V21h2v-3.28c.91-.13 1.77-.45 2.54-.9L19.73 21 21 19.73 4.27 3z"/>
          </svg>
        {:else}
          <svg width="18" height="18" viewBox="0 0 24 24" fill="currentColor">
            <path d="M12 14c1.66 0 3-1.34 3-3V5c0-1.66-1.34-3-3-3S9 3.34 9 5v6c0 1.66 1.34 3 3 3z"/>
            <path d="M17 11c0 2.76-2.24 5-5 5s-5-2.24-5-5H5c0 3.53 2.61 6.43 6 6.92V21h2v-3.08c3.39-.49 6-3.39 6-6.92h-2z"/>
          </svg>
        {/if}
      </button>

      <button
        class="control-btn"
        class:active={$voiceDeafened}
        onclick={toggleDeafen}
        title={$voiceDeafened ? "Undeafen" : "Deafen"}
      >
        {#if $voiceDeafened}
          <svg width="18" height="18" viewBox="0 0 24 24" fill="currentColor">
            <path d="M3.63 3.63a.996.996 0 000 1.41L7.29 8.7 7 9H4c-.55 0-1 .45-1 1v4c0 .55.45 1 1 1h3l3.29 3.29c.63.63 1.71.18 1.71-.71v-4.17l4.18 4.18c-.49.37-1.02.68-1.6.91-.36.15-.58.53-.58.92 0 .72.73 1.18 1.39.91.8-.33 1.55-.77 2.22-1.31l1.34 1.34a.996.996 0 101.41-1.41L5.05 3.63c-.39-.39-1.02-.39-1.42 0zM19 12c0 .82-.15 1.61-.41 2.34l1.53 1.53c.56-1.17.88-2.48.88-3.87 0-3.83-2.4-7.11-5.78-8.4-.59-.23-1.22.23-1.22.86v.19c0 .38.25.71.61.85C17.18 6.54 19 9.06 19 12zm-8.71-6.29l-.17.17L12 7.76V6.41c0-.89-1.08-1.33-1.71-.7zM16.5 12A4.5 4.5 0 0014 7.97v1.79l2.48 2.48c.01-.08.02-.16.02-.24z"/>
          </svg>
        {:else}
          <svg width="18" height="18" viewBox="0 0 24 24" fill="currentColor">
            <path d="M3 9v6h4l5 5V4L7 9H3zm13.5 3A4.5 4.5 0 0014 7.97v8.05c1.48-.73 2.5-2.25 2.5-3.02zM14 3.23v2.06c2.89.86 5 3.54 5 6.71s-2.11 5.85-5 6.71v2.06c4.01-.91 7-4.49 7-8.77s-2.99-7.86-7-8.77z"/>
          </svg>
        {/if}
      </button>

      <button
        class="control-btn disconnect"
        onclick={leaveVoice}
        title="Disconnect"
      >
        <svg width="18" height="18" viewBox="0 0 24 24" fill="currentColor">
          <path d="M12 9c-1.6 0-3.15.25-4.6.72v3.1c0 .39-.23.74-.56.9-.98.49-1.87 1.12-2.66 1.85-.18.18-.43.28-.7.28-.28 0-.53-.11-.71-.29L.29 13.08a.956.956 0 010-1.36C3.69 8.68 7.65 7 12 7s8.31 1.68 11.71 4.72c.38.37.38.98 0 1.36l-2.48 2.48c-.18.18-.43.29-.71.29-.27 0-.52-.11-.7-.28-.79-.73-1.68-1.36-2.66-1.85a.994.994 0 01-.56-.9v-3.1C15.15 9.25 13.6 9 12 9z"/>
        </svg>
      </button>
    </div>
  {/if}
</div>

<style>
  .voice-error {
    padding: 4px 8px;
    margin: 0 4px 4px;
    border-radius: 4px;
    background: rgba(239, 68, 68, 0.15);
    color: var(--error);
    font-size: 11px;
    line-height: 1.3;
  }

  .voice-section {
    margin-top: 8px;
  }

  .voice-header {
    font-size: 11px;
    font-weight: 700;
    color: var(--text-muted);
    letter-spacing: 0.04em;
    padding: 0 8px;
    margin-bottom: 6px;
  }

  .voice-channel {
    padding: 8px 10px;
    border-radius: 6px;
    color: var(--text-secondary);
    display: flex;
    align-items: center;
    gap: 6px;
    cursor: pointer;
    user-select: none;
  }

  .voice-channel:hover {
    background: var(--bg-tertiary);
    color: var(--text-primary);
  }

  .voice-channel.active {
    background: var(--bg-tertiary);
    color: var(--text-primary);
    cursor: default;
  }

  .voice-icon {
    flex-shrink: 0;
    opacity: 0.7;
  }

  .participants {
    padding: 2px 0 2px 20px;
  }

  .participant {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 3px 8px;
    border-radius: 4px;
    font-size: 13px;
    color: var(--text-secondary);
  }

  .participant-avatar {
    width: 24px;
    height: 24px;
    border-radius: 50%;
    background: var(--accent);
    display: flex;
    align-items: center;
    justify-content: center;
    font-weight: 700;
    font-size: 11px;
    color: white;
    flex-shrink: 0;
    transition: box-shadow 0.15s;
  }

  .participant-avatar.speaking {
    box-shadow: 0 0 0 2px var(--bg-secondary), 0 0 0 4px #43b581;
  }

  .participant-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }

  .status-icon {
    flex-shrink: 0;
    opacity: 0.6;
  }

  .status-icon.muted {
    color: var(--error);
  }

  .status-icon.deafened {
    color: var(--error);
  }

  .voice-controls {
    display: flex;
    gap: 4px;
    padding: 6px 8px;
    justify-content: center;
  }

  .control-btn {
    width: 36px;
    height: 36px;
    border-radius: 50%;
    border: none;
    background: var(--bg-tertiary);
    color: var(--text-secondary);
    display: flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
    transition: background-color 0.15s, color 0.15s;
  }

  .control-btn:hover {
    background: rgba(255, 255, 255, 0.1);
    color: var(--text-primary);
  }

  .control-btn.active {
    background: rgba(239, 68, 68, 0.2);
    color: var(--error);
  }

  .control-btn.active:hover {
    background: rgba(239, 68, 68, 0.3);
  }

  .control-btn.disconnect {
    background: rgba(239, 68, 68, 0.2);
    color: var(--error);
  }

  .control-btn.disconnect:hover {
    background: rgba(239, 68, 68, 0.4);
  }
</style>
