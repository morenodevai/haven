<script lang="ts">
  import { sendMessage } from "../../stores/messages";
  import EmojiPicker from "./EmojiPicker.svelte";

  let input = $state("");
  let sending = $state(false);
  let showEmoji = $state(false);
  let textareaEl: HTMLTextAreaElement;
  let fileInputEl: HTMLInputElement;

  // Staged image state
  let stagedImage: { dataUrl: string; base64: string; mime: string; name: string } | null = $state(null);

  async function handleSend() {
    if (sending) return;

    // Send staged image
    if (stagedImage) {
      sending = true;
      try {
        const envelope = JSON.stringify({
          type: "image",
          mime: stagedImage.mime,
          data: stagedImage.base64,
          name: stagedImage.name,
        });
        await sendMessage(envelope);
        stagedImage = null;
      } catch (e) {
        console.error("Failed to send image:", e);
      }
      sending = false;
      textareaEl?.focus();
      return;
    }

    // Send text
    const text = input.trim();
    if (!text) return;

    sending = true;
    try {
      await sendMessage(text);
      input = "";
    } catch (e) {
      console.error("Failed to send:", e);
    }
    sending = false;
    textareaEl?.focus();
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      handleSend();
    }
  }

  function insertEmoji(emoji: string) {
    input += emoji;
    textareaEl?.focus();
  }

  function handleWindowClick(e: MouseEvent) {
    if (showEmoji) {
      const anchor = (e.target as HTMLElement).closest('.emoji-anchor');
      if (!anchor) showEmoji = false;
    }
  }

  function openFilePicker() {
    fileInputEl?.click();
  }

  function cancelStagedImage() {
    stagedImage = null;
    if (fileInputEl) fileInputEl.value = "";
    textareaEl?.focus();
  }

  async function handleFileSelect(e: Event) {
    const target = e.target as HTMLInputElement;
    const file = target.files?.[0];
    if (!file) return;

    try {
      const { dataUrl, base64, mime } = await resizeImage(file);
      stagedImage = { dataUrl, base64, mime, name: file.name };
    } catch (err) {
      console.error("Failed to process image:", err);
    }
    // Reset so the same file can be re-selected
    target.value = "";
  }

  function resizeImage(file: File): Promise<{ dataUrl: string; base64: string; mime: string }> {
    return new Promise((resolve, reject) => {
      const reader = new FileReader();
      reader.onload = () => {
        const img = new Image();
        img.onload = () => {
          const MAX = 1920;
          let { width, height } = img;

          if (width > MAX || height > MAX) {
            if (width > height) {
              height = Math.round(height * (MAX / width));
              width = MAX;
            } else {
              width = Math.round(width * (MAX / height));
              height = MAX;
            }
          }

          const canvas = document.createElement("canvas");
          canvas.width = width;
          canvas.height = height;
          const ctx = canvas.getContext("2d")!;
          ctx.drawImage(img, 0, 0, width, height);

          const mime = "image/jpeg";
          const dataUrl = canvas.toDataURL(mime, 0.8);
          const base64 = dataUrl.split(",")[1];
          resolve({ dataUrl, base64, mime });
        };
        img.onerror = reject;
        img.src = reader.result as string;
      };
      reader.onerror = reject;
      reader.readAsDataURL(file);
    });
  }
</script>

<svelte:window onclick={handleWindowClick} />

<input
  type="file"
  accept="image/*"
  class="hidden-file-input"
  bind:this={fileInputEl}
  onchange={handleFileSelect}
/>

<div class="input-container">
  {#if stagedImage}
    <div class="image-preview">
      <img src={stagedImage.dataUrl} alt={stagedImage.name} />
      <button class="preview-cancel" onclick={cancelStagedImage} title="Remove image">✕</button>
    </div>
  {/if}
  <div class="input-wrapper">
    <div class="emoji-anchor">
      <button class="emoji-toggle" onclick={() => showEmoji = !showEmoji} title="Emoji">
        😀
      </button>
      {#if showEmoji}
        <EmojiPicker onSelect={insertEmoji} />
      {/if}
    </div>
    <button class="image-btn" onclick={openFilePicker} title="Send image" disabled={!!stagedImage}>
      <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <rect x="3" y="3" width="18" height="18" rx="2" ry="2"/>
        <circle cx="8.5" cy="8.5" r="1.5"/>
        <path d="M21 15l-5-5L5 21"/>
      </svg>
    </button>
    {#if stagedImage}
      <div class="staged-name">{stagedImage.name}</div>
    {:else}
      <textarea
        class="message-input"
        placeholder="Send an encrypted message..."
        bind:value={input}
        bind:this={textareaEl}
        onkeydown={handleKeydown}
        rows="1"
      ></textarea>
    {/if}
    <button class="send-btn" onclick={handleSend} disabled={(!input.trim() && !stagedImage) || sending}>
      <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
        <path d="M22 2L11 13M22 2l-7 20-4-9-9-4 20-7z"/>
      </svg>
    </button>
  </div>
  <div class="encrypt-badge">End-to-end encrypted</div>
</div>

<style>
  .hidden-file-input {
    display: none;
  }

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

  .emoji-anchor {
    position: relative;
  }

  .emoji-toggle {
    background: none;
    border: none;
    font-size: 20px;
    cursor: pointer;
    padding: 4px;
    border-radius: 6px;
    line-height: 1;
    transition: background-color 0.15s;
  }

  .emoji-toggle:hover {
    background: var(--bg-tertiary);
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

  .image-btn {
    background: none;
    border: none;
    color: var(--text-muted);
    cursor: pointer;
    padding: 4px;
    border-radius: 6px;
    line-height: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    transition: background-color 0.15s, color 0.15s;
  }

  .image-btn:hover:not(:disabled) {
    background: var(--bg-tertiary);
    color: var(--text-primary);
  }

  .image-btn:disabled {
    opacity: 0.3;
    cursor: default;
  }

  .staged-name {
    flex: 1;
    color: var(--text-muted);
    font-size: 13px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    align-self: center;
  }

  .image-preview {
    position: relative;
    display: inline-block;
    margin-bottom: 8px;
  }

  .image-preview img {
    max-width: 200px;
    max-height: 150px;
    border-radius: 8px;
    object-fit: cover;
    display: block;
  }

  .preview-cancel {
    position: absolute;
    top: 4px;
    right: 4px;
    width: 22px;
    height: 22px;
    border-radius: 50%;
    border: none;
    background: rgba(0, 0, 0, 0.6);
    color: white;
    font-size: 12px;
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    line-height: 1;
  }

  .preview-cancel:hover {
    background: rgba(0, 0, 0, 0.8);
  }

  .encrypt-badge {
    text-align: center;
    font-size: 11px;
    color: var(--text-muted);
    margin-top: 6px;
    user-select: none;
  }
</style>
