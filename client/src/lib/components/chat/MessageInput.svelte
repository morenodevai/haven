<script lang="ts">
  import { sendMessage, channelKey } from "../../stores/messages";
  import { get } from "svelte/store";
  import * as crypto from "../../ipc/crypto";
  import { uploadFile } from "../../ipc/api";
  import EmojiPicker from "./EmojiPicker.svelte";

  const MAX_VIDEO_SIZE = 50 * 1024 * 1024; // 50MB

  let input = $state("");
  let sending = $state(false);
  let showEmoji = $state(false);
  let errorToast: string | null = $state(null);
  let textareaEl: HTMLTextAreaElement;
  let fileInputEl: HTMLInputElement;

  // Staged image state
  let stagedImage: { dataUrl: string; base64: string; mime: string; name: string } | null = $state(null);
  // Staged video state
  let stagedVideo: { file: File; mime: string; name: string } | null = $state(null);

  function isVideoFile(file: File): boolean {
    return file.type.startsWith("video/");
  }

  function showError(msg: string) {
    errorToast = msg;
    setTimeout(() => { errorToast = null; }, 4000);
  }

  async function handleSend() {
    if (sending) return;

    // Send staged video via file upload
    if (stagedVideo) {
      sending = true;
      try {
        const key = get(channelKey);
        if (!key) throw new Error("No channel key set");

        // Read file as base64 using FileReader (handles any size)
        const dataUrl = await readFileAsDataUrl(stagedVideo.file);
        const fileBase64 = dataUrl.split(",")[1];

        // Encrypt the base64 string
        const encrypted = await crypto.encrypt(key, fileBase64);

        // Decode ciphertext from base64 to raw bytes for upload
        const binaryStr = atob(encrypted.ciphertext);
        const ctBytes = new Uint8Array(binaryStr.length);
        for (let i = 0; i < binaryStr.length; i++) {
          ctBytes[i] = binaryStr.charCodeAt(i);
        }

        // Upload encrypted blob
        const { file_id } = await uploadFile(ctBytes);

        // Send message envelope with file reference
        const envelope = JSON.stringify({
          type: "video",
          file_id,
          mime: stagedVideo.mime,
          name: stagedVideo.name,
          nonce: encrypted.nonce,
        });
        await sendMessage(envelope);
        stagedVideo = null;
      } catch (e) {
        console.error("Failed to send video:", e);
        showError("Failed to send video");
      }
      sending = false;
      textareaEl?.focus();
      return;
    }

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

  function cancelStaged() {
    stagedImage = null;
    stagedVideo = null;
    if (fileInputEl) fileInputEl.value = "";
    textareaEl?.focus();
  }

  async function handleFileSelect(e: Event) {
    const target = e.target as HTMLInputElement;
    const file = target.files?.[0];
    if (!file) return;

    if (isVideoFile(file)) {
      // Video file — stage for upload
      if (file.size > MAX_VIDEO_SIZE) {
        showError("Video too large (max 50 MB)");
        target.value = "";
        return;
      }
      stagedVideo = { file, mime: file.type, name: file.name };
      stagedImage = null;
    } else {
      // Image file — existing inline behavior
      try {
        const dataUrl = await readFileAsDataUrl(file);
        const base64 = dataUrl.split(",")[1];
        const mime = file.type || "image/png";
        stagedImage = { dataUrl, base64, mime, name: file.name };
        stagedVideo = null;
      } catch (err) {
        console.error("Failed to process image:", err);
      }
    }
    target.value = "";
  }

  function readFileAsDataUrl(file: File): Promise<string> {
    return new Promise((resolve, reject) => {
      const reader = new FileReader();
      reader.onload = () => resolve(reader.result as string);
      reader.onerror = reject;
      reader.readAsDataURL(file);
    });
  }
</script>

<svelte:window onclick={handleWindowClick} />

<input
  type="file"
  accept="image/*,image/gif,video/mp4,video/webm,video/quicktime"
  class="hidden-file-input"
  bind:this={fileInputEl}
  onchange={handleFileSelect}
/>

{#if errorToast}
  <div class="error-toast">{errorToast}</div>
{/if}

<div class="input-container">
  {#if stagedImage}
    <div class="image-preview">
      <img src={stagedImage.dataUrl} alt={stagedImage.name} />
      <button class="preview-cancel" onclick={cancelStaged} title="Remove image">✕</button>
    </div>
  {/if}
  {#if stagedVideo}
    <div class="video-staged">
      <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <polygon points="23 7 16 12 23 17 23 7"/>
        <rect x="1" y="5" width="15" height="14" rx="2" ry="2"/>
      </svg>
      <span class="staged-video-name">{stagedVideo.name}</span>
      <button class="preview-cancel" onclick={cancelStaged} title="Remove video">✕</button>
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
    <button class="image-btn" onclick={openFilePicker} title="Send image or video" disabled={!!stagedImage || !!stagedVideo}>
      <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <rect x="3" y="3" width="18" height="18" rx="2" ry="2"/>
        <circle cx="8.5" cy="8.5" r="1.5"/>
        <path d="M21 15l-5-5L5 21"/>
      </svg>
    </button>
    {#if stagedImage}
      <div class="staged-name">{stagedImage.name}</div>
    {:else if stagedVideo}
      <div class="staged-name">{stagedVideo.name}</div>
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
    <button class="send-btn" onclick={handleSend} disabled={(!input.trim() && !stagedImage && !stagedVideo) || sending}>
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

  .video-staged {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 12px;
    margin-bottom: 8px;
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: 8px;
    color: var(--text-primary);
    position: relative;
  }

  .staged-video-name {
    flex: 1;
    font-size: 13px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .error-toast {
    position: fixed;
    bottom: 80px;
    left: 50%;
    transform: translateX(-50%);
    background: var(--error, #ef4444);
    color: white;
    padding: 8px 16px;
    border-radius: 8px;
    font-size: 13px;
    font-weight: 600;
    z-index: 1000;
    animation: fadeIn 0.2s ease;
  }

  @keyframes fadeIn {
    from { opacity: 0; transform: translateX(-50%) translateY(8px); }
    to { opacity: 1; transform: translateX(-50%) translateY(0); }
  }
</style>
