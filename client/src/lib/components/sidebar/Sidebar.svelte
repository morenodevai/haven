<script lang="ts">
  import { auth, logout } from "../../stores/auth";
</script>

<div class="sidebar">
  <div class="server-header">
    <h2>Haven</h2>
  </div>

  <div class="channels">
    <div class="channel-header">TEXT CHANNELS</div>
    <div class="channel active">
      <span class="hash">#</span>
      general
    </div>
  </div>

  <div class="user-panel">
    <div class="user-info">
      <div class="user-avatar">
        {$auth.username?.charAt(0).toUpperCase() ?? "?"}
      </div>
      <div class="user-name">{$auth.username}</div>
    </div>
    <button class="logout-btn" onclick={logout} title="Log out">
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
        <path d="M9 21H5a2 2 0 01-2-2V5a2 2 0 012-2h4M16 17l5-5-5-5M21 12H9"/>
      </svg>
    </button>
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
    cursor: default;
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

  .logout-btn {
    background: none;
    border: none;
    color: var(--text-muted);
    padding: 6px;
    border-radius: 4px;
    display: flex;
    align-items: center;
  }

  .logout-btn:hover {
    color: var(--error);
    background: rgba(239, 68, 68, 0.1);
  }
</style>
