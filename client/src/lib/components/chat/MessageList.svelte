<script lang="ts">
  import { messages, type DecryptedMessage } from "../../stores/messages";
  import { auth } from "../../stores/auth";
  import { tick } from "svelte";

  let container: HTMLDivElement;

  // Auto-scroll to bottom when new messages arrive
  $effect(() => {
    if ($messages.length > 0) {
      tick().then(() => {
        if (container) {
          container.scrollTop = container.scrollHeight;
        }
      });
    }
  });

  function formatTime(timestamp: string): string {
    try {
      const date = new Date(timestamp);
      return date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
    } catch {
      return "";
    }
  }

  function isOwnMessage(msg: DecryptedMessage): boolean {
    return msg.authorId === $auth.userId;
  }
</script>

<div class="message-list" bind:this={container}>
  {#if $messages.length === 0}
    <div class="empty">
      <div class="empty-icon">#</div>
      <h2>Welcome to #general</h2>
      <p>This is the start of encrypted history. Send a message!</p>
    </div>
  {:else}
    {#each $messages as msg (msg.id)}
      <div class="message" class:own={isOwnMessage(msg)}>
        <div class="avatar">
          {msg.authorUsername.charAt(0).toUpperCase()}
        </div>
        <div class="message-body">
          <div class="message-header">
            <span class="author" class:self={isOwnMessage(msg)}>
              {msg.authorUsername}
            </span>
            <span class="time">{formatTime(msg.timestamp)}</span>
          </div>
          <div class="content">{msg.content}</div>
        </div>
      </div>
    {/each}
  {/if}
</div>

<style>
  .message-list {
    flex: 1;
    overflow-y: auto;
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .empty {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    color: var(--text-muted);
  }

  .empty-icon {
    font-size: 48px;
    font-weight: 700;
    color: var(--text-muted);
    margin-bottom: 12px;
  }

  .empty h2 {
    color: var(--text-primary);
    margin-bottom: 4px;
  }

  .message {
    display: flex;
    gap: 12px;
    padding: 6px 8px;
    border-radius: 6px;
    transition: background-color 0.1s;
  }

  .message:hover {
    background: var(--message-hover);
  }

  .avatar {
    width: 36px;
    height: 36px;
    border-radius: 50%;
    background: var(--accent);
    display: flex;
    align-items: center;
    justify-content: center;
    font-weight: 700;
    font-size: 14px;
    color: white;
    flex-shrink: 0;
    margin-top: 2px;
  }

  .message-body {
    min-width: 0;
    flex: 1;
  }

  .message-header {
    display: flex;
    align-items: baseline;
    gap: 8px;
  }

  .author {
    font-weight: 600;
    font-size: 14px;
  }

  .author.self {
    color: var(--accent);
  }

  .time {
    font-size: 11px;
    color: var(--text-muted);
  }

  .content {
    color: var(--text-primary);
    word-break: break-word;
    white-space: pre-wrap;
  }
</style>
