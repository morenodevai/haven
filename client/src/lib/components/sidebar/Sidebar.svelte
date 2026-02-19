<script lang="ts">
  import { auth, logout } from "../../stores/auth";
  import { activeChannel } from "../../stores/channels";
  import { checkForUpdate, installUpdate, updateAvailable, updateVersion, updateProgress } from "../../stores/updater";
  import VoiceChannel from "../voice/VoiceChannel.svelte";

  async function handleUpdateClick() {
    if ($updateAvailable) {
      await installUpdate();
    } else {
      await checkForUpdate();
    }
  }
</script>

<div class="sidebar">
  <div class="server-header">
    <h2>Haven</h2>
  </div>

  <div class="channels">
    <div class="channel-header">TEXT CHANNELS</div>
    <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
    <div class="channel" class:active={$activeChannel === "general"} onclick={() => activeChannel.set("general")}>
      <span class="hash">#</span>
      general
    </div>

    <div class="channel-header file-header">FILE SHARING</div>
    <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
    <div class="channel" class:active={$activeChannel === "file-sharing"} onclick={() => activeChannel.set("file-sharing")}>
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <path d="M13 2H6a2 2 0 00-2 2v16a2 2 0 002 2h12a2 2 0 002-2V9z"/>
        <polyline points="13 2 13 9 20 9"/>
      </svg>
      file-sharing
    </div>

    <VoiceChannel />
  </div>

  <div class="user-panel">
    <div class="user-info">
      <div class="user-avatar">
        {$auth.username?.charAt(0).toUpperCase() ?? "?"}
      </div>
      <div class="user-name">{$auth.username}</div>
    </div>
    <div class="panel-actions">
      <button
        class="icon-btn update-btn"
        class:has-update={$updateAvailable}
        onclick={handleUpdateClick}
        disabled={$updateProgress === "downloading" || $updateProgress === "installing"}
        title={$updateAvailable ? `Update to ${$updateVersion}` : $updateProgress === "checking" ? "Checking..." : "Check for updates"}
      >
        {#if $updateProgress === "checking" || $updateProgress === "downloading" || $updateProgress === "installing"}
          <svg class="spin" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M21 12a9 9 0 11-6.219-8.56"/></svg>
        {:else if $updateAvailable}
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M21 15v4a2 2 0 01-2 2H5a2 2 0 01-2-2v-4"/>
            <polyline points="7 10 12 15 17 10"/>
            <line x1="12" y1="15" x2="12" y2="3"/>
          </svg>
        {:else}
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <polyline points="23 4 23 10 17 10"/>
            <path d="M20.49 15a9 9 0 11-2.12-9.36L23 10"/>
          </svg>
        {/if}
      </button>
      <button class="icon-btn logout-btn" onclick={logout} title="Log out">
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <path d="M9 21H5a2 2 0 01-2-2V5a2 2 0 012-2h4M16 17l5-5-5-5M21 12H9"/>
        </svg>
      </button>
    </div>
  </div>
</div>

<style>
  .sidebar {
    width: 240px;
    background: var(--bg-secondary);
    display: flex;
    flex-direction: column;
    border-right: 1px solid var(--border);
    flex-shrink: 0;
  }

  .server-header {
    padding: 14px 16px;
    border-bottom: 1px solid var(--border);
  }

  .server-header h2 {
    font-size: 16px;
    font-weight: 700;
  }

  .channels {
    flex: 1;
    padding: 12px 8px;
    overflow-y: auto;
  }

  .channel-header {
    font-size: 11px;
    font-weight: 700;
    color: var(--text-muted);
    letter-spacing: 0.04em;
    padding: 0 8px;
    margin-bottom: 6px;
  }

  .channel {
    padding: 8px 10px;
    border-radius: 6px;
    color: var(--text-secondary);
    display: flex;
    align-items: center;
    gap: 6px;
    cursor: pointer;
  }

  .file-header {
    margin-top: 14px;
  }

  .channel.active {
    background: var(--bg-tertiary);
    color: var(--text-primary);
  }

  .hash {
    color: var(--text-muted);
    font-weight: 600;
  }

  .user-panel {
    padding: 10px 12px;
    border-top: 1px solid var(--border);
    background: rgba(0, 0, 0, 0.15);
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  .user-info {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }

  .user-avatar {
    width: 32px;
    height: 32px;
    border-radius: 50%;
    background: var(--accent);
    display: flex;
    align-items: center;
    justify-content: center;
    font-weight: 700;
    font-size: 13px;
    color: white;
    flex-shrink: 0;
  }

  .user-name {
    font-size: 13px;
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .panel-actions {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .icon-btn {
    background: none;
    border: none;
    color: var(--text-muted);
    padding: 6px;
    border-radius: 4px;
    display: flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
  }

  .icon-btn:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .update-btn:hover:not(:disabled) {
    color: var(--accent);
    background: rgba(88, 101, 242, 0.1);
  }

  .update-btn.has-update {
    color: var(--accent);
  }

  .logout-btn:hover {
    color: var(--error);
    background: rgba(239, 68, 68, 0.1);
  }

  .spin {
    animation: spin 1s linear infinite;
  }

  @keyframes spin {
    from { transform: rotate(0deg); }
    to { transform: rotate(360deg); }
  }
</style>
