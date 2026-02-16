<script lang="ts">
  import { isCustomLetter, getLetterChar, isLetterWord, parseLetters } from "../../utils/emoji";

  let {
    emoji,
    count,
    highlighted,
    onclick,
  }: {
    emoji: string;
    count: number;
    highlighted: boolean;
    onclick: () => void;
  } = $props();
</script>

<button class="reaction-badge" class:highlighted class:letter-badge={isCustomLetter(emoji) || isLetterWord(emoji)} onclick={onclick} title={emoji}>
  {#if isLetterWord(emoji)}
    {#each parseLetters(emoji) as letter}
      <span class="badge-letter">{letter}</span>
    {/each}
  {:else if isCustomLetter(emoji)}
    <span class="badge-letter">{getLetterChar(emoji)}</span>
  {:else}
    <span class="badge-emoji">{emoji}</span>
  {/if}
  <span class="badge-count">{count}</span>
</button>

<style>
  .reaction-badge {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 2px 8px;
    border-radius: 12px;
    border: 1px solid var(--border);
    background: var(--bg-secondary);
    cursor: pointer;
    font-size: 14px;
    line-height: 1.4;
    transition: background-color 0.15s, border-color 0.15s;
  }

  .reaction-badge:hover {
    background: var(--bg-tertiary);
  }

  .reaction-badge.highlighted {
    border-color: var(--accent);
    background: rgba(88, 101, 242, 0.15);
  }

  .badge-emoji {
    font-size: 16px;
    line-height: 1;
  }

  .reaction-badge.letter-badge {
    background: #5865f2;
    border-color: #5865f2;
  }

  .reaction-badge.letter-badge:hover {
    background: #4752c4;
  }

  .reaction-badge.letter-badge.highlighted {
    border-color: var(--accent);
    background: #4752c4;
  }

  .badge-letter {
    color: white;
    font-weight: 700;
    font-size: 14px;
    font-family: 'Segoe UI', sans-serif;
    line-height: 1;
  }

  .badge-count {
    font-size: 12px;
    font-weight: 600;
    color: var(--text-secondary);
  }

  .reaction-badge.letter-badge .badge-count {
    color: rgba(255, 255, 255, 0.85);
  }

  .reaction-badge.highlighted .badge-count {
    color: var(--accent);
  }

  .reaction-badge.letter-badge.highlighted .badge-count {
    color: white;
  }
</style>
