<script lang="ts">
  import { auth } from "../../stores/auth";
  import { onlineUserList } from "../../stores/presence";
  import {
    transfers,
    pendingOffers,
    activeTransfers,
    sendFile,
    acceptTransfer,
    rejectTransfer,
    cancelTransfer,
    formatBytes,
    formatSpeed,
    type Transfer,
  } from "../../stores/transfers";

  let dragOver = $state(false);
  let selectedPeer: { id: string; username: string } | null = $state(null);
  let fileInputEl: HTMLInputElement;

  // Filter out self from online users
  const otherUsers = $derived(
    $onlineUserList.filter((u) => u.userId !== $auth.userId)
  );

  function handleDrop(e: DragEvent) {
    e.preventDefault();
    dragOver = false;
    if (!selectedPeer) return;
    const files = e.dataTransfer?.files;
    if (files) {
      for (const file of files) {
        sendFile(selectedPeer.id, selectedPeer.username, file);
      }
    }
  }

  function handleDragOver(e: DragEvent) {
    e.preventDefault();
    dragOver = true;
  }

  function handleDragLeave() {
    dragOver = false;
  }

  function openFilePicker(peerId: string, peerUsername: string) {
    selectedPeer = { id: peerId, username: peerUsername };
    fileInputEl?.click();
  }

  function handleFileSelect(e: Event) {
    const target = e.target as HTMLInputElement;
    const files = target.files;
    if (!files || !selectedPeer) return;
    for (const file of files) {
      sendFile(selectedPeer.id, selectedPeer.username, file);
    }
    target.value = "";
  }

  function progressPercent(t: Transfer): number {
    if (t.size === 0) return 100;
    return Math.round((t.bytesTransferred / t.size) * 100);
  }

  function peerName(t: Transfer): string {
    // Try to resolve from online users
    const user = $onlineUserList.find((u) => u.userId === t.peerId);
    return user?.username || t.peerUsername;
  }

  function statusLabel(t: Transfer): string {
    switch (t.status) {
      case "pending":
        return t.direction === "receive" ? "Incoming..." : "Waiting...";
      case "connecting":
        return "Connecting...";
      case "transferring":
        return `${progressPercent(t)}%  ${formatBytes(t.bytesTransferred)} / ${formatBytes(t.size)}`;
      case "completed":
        return "Completed";
      case "failed":
        return "Failed";
      case "rejected":
        return "Rejected";
      case "cancelled":
        return "Cancelled";
      default:
        return t.status;
    }
  }
</script>

<input
  type="file"
  class="hidden-file-input"
  bind:this={fileInputEl}
  onchange={handleFileSelect}
  multiple
/>

<div class="file-channel">
  <div class="panels">
    <!-- Online users panel -->
    <div class="panel users-panel">
      <div class="panel-title">ONLINE USERS</div>
      {#if otherUsers.length === 0}
        <div class="empty-state">No other users online</div>
      {:else}
        {#each otherUsers as user}
          <div class="user-row">
            <div class="user-status-dot"></div>
            <span class="user-name">{user.username}</span>
            <button
              class="send-btn"
              onclick={() => openFilePicker(user.userId, user.username)}
            >
              Send File
            </button>
          </div>
        {/each}
      {/if}
    </div>

    <!-- Transfers panel -->
    <div class="panel transfers-panel">
      <div class="panel-title">TRANSFERS</div>

      {#if $pendingOffers.length > 0}
        <div class="section-label">Incoming Requests</div>
        {#each $pendingOffers as offer}
          <div class="transfer-card pending">
            <div class="transfer-info">
              <div class="transfer-filename">{offer.filename}</div>
              <div class="transfer-meta">
                {formatBytes(offer.size)} from {peerName(offer)}
              </div>
            </div>
            <div class="transfer-actions">
              <button
                class="accept-btn"
                onclick={() => acceptTransfer(offer.id)}
              >
                Accept
              </button>
              <button
                class="reject-btn"
                onclick={() => rejectTransfer(offer.id)}
              >
                Reject
              </button>
            </div>
          </div>
        {/each}
      {/if}

      {#if $transfers.filter((t) => t.status !== "pending" || t.direction === "send").length > 0}
        <div class="section-label">All Transfers</div>
      {/if}
      {#each $transfers.filter((t) => t.status !== "pending" || t.direction === "send") as t}
        <div class="transfer-card {t.status}">
          <div class="transfer-info">
            <div class="transfer-filename">
              {t.direction === "send" ? "↑" : "↓"}
              {t.filename}
            </div>
            <div class="transfer-meta">
              {statusLabel(t)}
              {#if t.status === "transferring" && t.startTime}
                <span class="transfer-speed">
                  {formatSpeed(t.bytesTransferred, t.startTime)}
                </span>
              {/if}
            </div>
            {#if t.status === "transferring"}
              <div class="progress-bar">
                <div
                  class="progress-fill"
                  style="width: {progressPercent(t)}%"
                ></div>
              </div>
            {/if}
          </div>
          {#if t.status === "transferring" || t.status === "connecting"}
            <button
              class="cancel-btn"
              onclick={() => cancelTransfer(t.id)}
              title="Cancel"
            >
              ✕
            </button>
          {/if}
        </div>
      {/each}

      {#if $transfers.length === 0}
        <div class="empty-state">No transfers yet</div>
      {/if}
    </div>
  </div>

  <!-- Drop zone -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="drop-zone"
    class:active={dragOver}
    ondrop={handleDrop}
    ondragover={handleDragOver}
    ondragleave={handleDragLeave}
  >
    {#if !selectedPeer && otherUsers.length > 0}
      Select a user above, then drop files here
    {:else if otherUsers.length === 0}
      Waiting for other users to come online...
    {:else}
      Drop files here to send to {selectedPeer?.username}
    {/if}
  </div>
</div>

<style>
  .hidden-file-input {
    display: none;
  }

  .file-channel {
    flex: 1;
    display: flex;
    flex-direction: column;
    padding: 16px;
    gap: 16px;
    overflow-y: auto;
  }

  .panels {
    display: flex;
    gap: 16px;
    flex: 1;
    min-height: 0;
  }

  .panel {
    flex: 1;
    background: var(--bg-secondary);
    border-radius: 8px;
    padding: 12px;
    overflow-y: auto;
  }

  .panel-title {
    font-size: 11px;
    font-weight: 700;
    color: var(--text-muted);
    letter-spacing: 0.04em;
    margin-bottom: 10px;
  }

  .section-label {
    font-size: 10px;
    font-weight: 600;
    color: var(--text-muted);
    text-transform: uppercase;
    letter-spacing: 0.03em;
    margin: 10px 0 6px;
  }

  .empty-state {
    color: var(--text-muted);
    font-size: 13px;
    padding: 16px 0;
    text-align: center;
  }

  /* Users */
  .user-row {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 8px;
    border-radius: 6px;
  }

  .user-row:hover {
    background: var(--bg-tertiary);
  }

  .user-status-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--success, #22c55e);
    flex-shrink: 0;
  }

  .user-name {
    flex: 1;
    font-size: 14px;
    font-weight: 500;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .send-btn {
    background: var(--accent);
    border: none;
    color: white;
    border-radius: 4px;
    padding: 4px 10px;
    font-size: 11px;
    font-weight: 600;
    cursor: pointer;
    flex-shrink: 0;
  }

  .send-btn:hover {
    opacity: 0.9;
  }

  /* Transfers */
  .transfer-card {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 10px;
    border-radius: 6px;
    background: var(--bg-tertiary);
    margin-bottom: 6px;
  }

  .transfer-info {
    flex: 1;
    min-width: 0;
  }

  .transfer-filename {
    font-size: 13px;
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .transfer-meta {
    font-size: 11px;
    color: var(--text-muted);
    margin-top: 2px;
  }

  .transfer-speed {
    color: var(--accent);
    margin-left: 6px;
  }

  .transfer-actions {
    display: flex;
    gap: 4px;
    flex-shrink: 0;
  }

  .accept-btn {
    background: var(--success, #22c55e);
    border: none;
    color: white;
    border-radius: 4px;
    padding: 4px 10px;
    font-size: 11px;
    font-weight: 600;
    cursor: pointer;
  }

  .reject-btn,
  .cancel-btn {
    background: none;
    border: 1px solid var(--text-muted);
    color: var(--text-muted);
    border-radius: 4px;
    padding: 4px 8px;
    font-size: 11px;
    cursor: pointer;
  }

  .reject-btn:hover,
  .cancel-btn:hover {
    border-color: var(--error, #ef4444);
    color: var(--error, #ef4444);
  }

  .progress-bar {
    height: 4px;
    background: rgba(255, 255, 255, 0.1);
    border-radius: 2px;
    margin-top: 6px;
    overflow: hidden;
  }

  .progress-fill {
    height: 100%;
    background: var(--accent);
    border-radius: 2px;
    transition: width 0.2s ease;
  }

  .transfer-card.completed .transfer-filename {
    color: var(--success, #22c55e);
  }

  .transfer-card.failed .transfer-filename,
  .transfer-card.rejected .transfer-filename {
    color: var(--error, #ef4444);
  }

  /* Drop zone */
  .drop-zone {
    border: 2px dashed var(--border);
    border-radius: 8px;
    padding: 20px;
    text-align: center;
    color: var(--text-muted);
    font-size: 13px;
    transition: all 0.2s;
    flex-shrink: 0;
  }

  .drop-zone.active {
    border-color: var(--accent);
    background: rgba(88, 101, 242, 0.05);
    color: var(--accent);
  }
</style>
