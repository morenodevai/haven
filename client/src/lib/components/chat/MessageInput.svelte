<script lang="ts">
  import { sendMessage } from "../../stores/messages";

  let input = $state("");
  let sending = $state(false);

  async function handleSend() {
    const text = input.trim();
    if (!text || sending) return;

    sending = true;
    try {
      await sendMessage(text);
      input = "";
    } catch (e) {
      console.error("Failed to send:", e);
    }
    sending = false;
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      handleSend();
    }
  }
</script>

<div class="input-container">
  <div class="input-wrapper">
    <textarea
      class="message-input"
      placeholder="Send an encrypted message..."
      bind:value={input}
      onkeydown={handleKeydown}
      rows="1"
      disabled={sending}
    ></textarea>
    <button class="send-btn" onclick={handleSend} disabled={!input.trim() || sending}>
      <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
        <path d="M22 2L11 13M22 2l-7 20-4-9-9-4 20-7z"/>
      </svg>
    </button>
  </div>
  <div class="encrypt-badge">End-to-end encrypted</div>
</div>

<style>
  .input-container {
    padding: 0 16px 16px;
  }

  .input-wrapper {
    display: flex;
    align-items: flex-end;
    gap: 8px;
    background: var(--bg-input);
    border: 1px solid var(--border);
    border-radius: 10px;
    padding: 10px 12px;
    transition: border-color 0.2s;
  }

  .input-wrapper:focus-within {
    border-color: var(--accent);
  }

  .message-input {
    flex: 1;
    background: none;
    border: none;
    color: var(--text-primary);
    outline: none;
    resize: none;
    max-height: 120px;
    line-height: 1.4;
  }

  .message-input::placeholder {
    color: var(--text-muted);
  }

  .send-btn {
    background: var(--accent);
    border: none;
    border-radius: 8px;
    padding: 6px 8px;
    color: white;
    display: flex;
    align-items: center;
    justify-content: center;
    transition: opacity 0.2s;
  }

  .send-btn:disabled {
    opacity: 0.3;
  }

  .send-btn:hover:not(:disabled) {
    background: var(--accent-hover);
  }

  .encrypt-badge {
    text-align: center;
    font-size: 11px;
    color: var(--text-muted);
    margin-top: 6px;
    user-select: none;
  }
</style>
