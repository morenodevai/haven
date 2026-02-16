<script lang="ts">
  import { messages, toggleReaction, type DecryptedMessage } from "../../stores/messages";
  import { auth } from "../../stores/auth";
  import { tick } from "svelte";
  import { isCustomLetter, getLetterChar, parseLetters } from "../../utils/emoji";
  import ReactionBadge from "./ReactionBadge.svelte";
  import EmojiPicker from "./EmojiPicker.svelte";

  let container: HTMLDivElement;
  let reactionPickerMsgId: string | null = $state(null);
  let letterBuffer: string = $state("");
  let lightboxSrc: string | null = $state(null);

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

  function hasReacted(msg: DecryptedMessage, emoji: string): boolean {
    const group = msg.reactions.find((r) => r.emoji === emoji);
    return group ? group.user_ids.includes($auth.userId ?? "") : false;
  }

  function handleToggleReaction(messageId: string, emoji: string) {
    toggleReaction(messageId, emoji);
  }

  function openReactionPicker(msgId: string) {
    reactionPickerMsgId = reactionPickerMsgId === msgId ? null : msgId;
  }

  function handleReactionSelect(emoji: string) {
    if (!reactionPickerMsgId) return;
    if (isCustomLetter(emoji)) {
      letterBuffer += emoji;
    } else {
      toggleReaction(reactionPickerMsgId, emoji);
      letterBuffer = "";
      reactionPickerMsgId = null;
    }
  }

  function sendWordReaction() {
    if (reactionPickerMsgId && letterBuffer.length > 0) {
      toggleReaction(reactionPickerMsgId, letterBuffer);
      letterBuffer = "";
      reactionPickerMsgId = null;
    }
  }

  function clearLetterBuffer() {
    letterBuffer = "";
  }

  function handleWindowClick(e: MouseEvent) {
    if (reactionPickerMsgId) {
      const target = e.target as HTMLElement;
      if (!target.closest(".reaction-picker-anchor") && !target.closest(".emoji-picker")) {
        letterBuffer = "";
        reactionPickerMsgId = null;
      }
    }
  }

  // Split message content into text and custom letter segments
  type Segment = { type: "text"; value: string } | { type: "letter"; value: string };
  function parseContent(text: string): Segment[] {
    const parts: Segment[] = [];
    const re = /\[:.\:]/g;
    let last = 0;
    let match;
    while ((match = re.exec(text)) !== null) {
      if (match.index > last) {
        parts.push({ type: "text", value: text.slice(last, match.index) });
      }
      parts.push({ type: "letter", value: match[0] });
      last = re.lastIndex;
    }
    if (last < text.length) {
      parts.push({ type: "text", value: text.slice(last) });
    }
    return parts;
  }
</script>

<svelte:window onclick={handleWindowClick} />

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
          <div class="bubble" class:own-bubble={isOwnMessage(msg)}>
            {#if msg.imageData}
              <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_noninteractive_element_interactions -->
              <img
                class="chat-image"
                src={msg.imageData}
                alt={msg.imageName || "image"}
                onclick={() => lightboxSrc = msg.imageData!}
              />
            {:else}
              <div class="content">{#each parseContent(msg.content) as seg}{#if seg.type === "letter"}<span class="letter-square">{getLetterChar(seg.value)}</span>{:else}{seg.value}{/if}{/each}</div>
            {/if}
          </div>
          <div class="reactions-row">
            {#each msg.reactions as reaction}
              <ReactionBadge
                emoji={reaction.emoji}
                count={reaction.count}
                highlighted={hasReacted(msg, reaction.emoji)}
                onclick={() => handleToggleReaction(msg.id, reaction.emoji)}
              />
            {/each}
            <div class="reaction-picker-anchor">
              <button
                class="add-reaction-btn"
                title="Add Reaction"
                onclick={(e) => { e.stopPropagation(); openReactionPicker(msg.id); }}
              >
                <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                  <circle cx="12" cy="12" r="10"/>
                  <path d="M8 14s1.5 2 4 2 4-2 4-2"/>
                  <line x1="9" y1="9" x2="9.01" y2="9"/>
                  <line x1="15" y1="9" x2="15.01" y2="9"/>
                </svg>
                <span class="plus-icon">+</span>
              </button>
              {#if reactionPickerMsgId === msg.id}
                <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
                <div class="picker-wrapper" onclick={(e) => e.stopPropagation()}>
                  <EmojiPicker onSelect={handleReactionSelect} />
                  {#if letterBuffer.length > 0}
                    <div class="word-preview-bar">
                      <div class="word-preview-letters">
                        {#each parseLetters(letterBuffer) as letter}
                          <span class="preview-letter-square">{letter}</span>
                        {/each}
                      </div>
                      <button class="word-clear-btn" onclick={clearLetterBuffer} title="Clear">✕</button>
                      <button class="word-react-btn" onclick={sendWordReaction}>React</button>
                    </div>
                  {/if}
                </div>
              {/if}
            </div>
          </div>
        </div>
      </div>
    {/each}
  {/if}
</div>

{#if lightboxSrc}
  <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
  <div class="lightbox-backdrop" onclick={() => lightboxSrc = null}>
    <img class="lightbox-image" src={lightboxSrc} alt="Full size" />
  </div>
{/if}

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

  .bubble {
    display: inline-block;
    max-width: 100%;
    background: var(--bg-secondary);
    border-radius: 12px;
    padding: 8px 12px;
    margin-top: 4px;
  }

  .bubble.own-bubble {
    background: rgba(88, 101, 242, 0.15);
  }

  .content {
    color: var(--text-primary);
    word-break: break-word;
    white-space: pre-wrap;
  }

  .chat-image {
    max-width: 400px;
    max-height: 300px;
    border-radius: 8px;
    cursor: pointer;
    display: block;
    object-fit: contain;
  }

  .chat-image:hover {
    opacity: 0.9;
  }

  .lightbox-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.85);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
    cursor: pointer;
  }

  .lightbox-image {
    max-width: 90vw;
    max-height: 90vh;
    object-fit: contain;
    border-radius: 4px;
  }

  .reactions-row {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    margin-top: 4px;
    min-height: 24px;
  }

  .reaction-picker-anchor {
    position: relative;
  }

  .add-reaction-btn {
    display: inline-flex;
    align-items: center;
    gap: 2px;
    padding: 2px 8px;
    border-radius: 12px;
    border: 1px dashed var(--border);
    background: transparent;
    cursor: pointer;
    color: var(--text-muted);
    font-size: 12px;
    font-weight: 600;
    transition: background-color 0.15s, border-color 0.15s;
  }

  .add-reaction-btn:hover {
    background: var(--bg-tertiary);
    border-color: var(--text-muted);
    color: var(--text-primary);
  }

  .plus-icon {
    font-size: 14px;
    line-height: 1;
  }

  .picker-wrapper {
    position: absolute;
    bottom: 100%;
    left: 0;
    margin-bottom: 4px;
    z-index: 100;
  }

  .letter-square {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 22px;
    height: 22px;
    background: #5865f2;
    border-radius: 4px;
    color: white;
    font-weight: 700;
    font-size: 13px;
    font-family: 'Segoe UI', sans-serif;
    vertical-align: middle;
    margin: 0 1px;
  }

  .word-preview-bar {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 8px;
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-top: none;
    border-radius: 0 0 10px 10px;
  }

  .word-preview-letters {
    display: flex;
    gap: 2px;
    flex: 1;
    min-width: 0;
    flex-wrap: wrap;
  }

  .preview-letter-square {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 22px;
    height: 22px;
    background: #5865f2;
    border-radius: 4px;
    color: white;
    font-weight: 700;
    font-size: 13px;
    font-family: 'Segoe UI', sans-serif;
  }

  .word-clear-btn {
    background: none;
    border: none;
    color: var(--text-muted);
    cursor: pointer;
    font-size: 14px;
    padding: 2px 4px;
    border-radius: 4px;
    line-height: 1;
  }

  .word-clear-btn:hover {
    color: var(--text-primary);
    background: var(--bg-tertiary);
  }

  .word-react-btn {
    background: #5865f2;
    border: none;
    color: white;
    font-weight: 600;
    font-size: 12px;
    padding: 4px 12px;
    border-radius: 6px;
    cursor: pointer;
    white-space: nowrap;
  }

  .word-react-btn:hover {
    background: #4752c4;
  }
</style>
