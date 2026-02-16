<script lang="ts">
  let { onSelect }: { onSelect: (emoji: string) => void } = $props();

  let activeCategory = $state("history");

  const HISTORY_KEY = "haven_emoji_history";
  const MAX_HISTORY = 40;

  function getHistory(): string[] {
    try {
      const raw = localStorage.getItem(HISTORY_KEY);
      return raw ? JSON.parse(raw) : [];
    } catch { return []; }
  }

  function addToHistory(emoji: string) {
    const hist = getHistory().filter(e => e !== emoji);
    hist.unshift(emoji);
    if (hist.length > MAX_HISTORY) hist.length = MAX_HISTORY;
    localStorage.setItem(HISTORY_KEY, JSON.stringify(hist));
    history = hist;
  }

  let history = $state(getHistory());

  import { isCustomLetter, getLetterChar } from "../../utils/emoji";

  // Custom letter emojis stored as [:A:] through [:Z:] and [:0:] through [:9:]
  const LETTERS = "ABCDEFGHIJKLMNOPQRSTUVWXYZ".split("").map(c => `[:${c}:]`);
  const DIGITS = "0123456789".split("").map(c => `[:${c}:]`);
  const CUSTOM_LETTERS = [...LETTERS, ...DIGITS];

  const categories: Record<string, { label: string; emojis: string[] }> = {
    history: {
      label: "🕐",
      emojis: [],
    },
    smileys: {
      label: "😀",
      emojis: [
        "😀", "😁", "😂", "🤣", "😃", "😄", "😅", "😆", "😉", "😊",
        "😋", "😎", "🤩", "😍", "🥰", "😘", "😗", "😙", "🥲", "😚",
        "😜", "🤪", "😝", "🤑", "🤗", "🤭", "🫢", "🤫", "🤔", "🫡",
        "🤐", "🤨", "😐", "😑", "😶", "🫥", "😏", "😒", "🙄", "😬",
        "😮‍💨", "🤥", "🫠", "😌", "😔", "😪", "🤤", "😴", "😷", "🤒",
        "🤕", "🤢", "🤮", "🥵", "🥶", "🥴", "😵", "🤯", "🤠", "🥳",
        "🥸", "😎", "🤓", "🧐", "😕", "🫤", "😟", "🙁", "😮", "😯",
        "😲", "😳", "🥺", "🥹", "😦", "😧", "😨", "😰", "😥", "😢",
        "😭", "😱", "😖", "😣", "😞", "😓", "😩", "😫", "🥱", "😤",
        "😡", "😠", "🤬", "😈", "👿", "💀", "☠️", "💩", "🤡", "👹",
      ],
    },
    gestures: {
      label: "👋",
      emojis: [
        "👋", "🤚", "🖐️", "✋", "🖖", "🫱", "🫲", "🫳", "🫴", "🫷",
        "🫸", "👌", "🤌", "🤏", "✌️", "🤞", "🫰", "🤟", "🤘", "🤙",
        "👈", "👉", "👆", "🖕", "👇", "☝️", "🫵", "👍", "👎", "✊",
        "👊", "🤛", "🤜", "👏", "🙌", "🫶", "👐", "🤲", "🤝", "🙏",
        "💪", "🦾", "🖤", "❤️", "🧡", "💛", "💚", "💙", "💜", "🤎",
        "🩷", "🩵", "🩶", "🖤", "🤍", "💔", "❤️‍🔥", "❤️‍🩹", "💕", "💞",
      ],
    },
    objects: {
      label: "🎮",
      emojis: [
        "🎮", "🕹️", "🎲", "🎯", "🏆", "🎪", "🎭", "🎨", "🎬", "🎤",
        "🎧", "🎵", "🎶", "🎸", "🥁", "🎹", "🎺", "🎻", "🪗", "🎷",
        "💻", "🖥️", "⌨️", "🖱️", "💾", "📱", "📲", "☎️", "📞", "📟",
        "🔋", "🔌", "💡", "🔦", "🕯️", "🪫", "📷", "📸", "📹", "🎥",
        "🔍", "🔎", "🔬", "🔭", "📡", "🛰️", "💣", "🔫", "🗡️", "⚔️",
        "🛡️", "🔧", "🪛", "🔩", "⚙️", "🪤", "📦", "📫", "📬", "✉️",
      ],
    },
    food: {
      label: "🍕",
      emojis: [
        "🍕", "🍔", "🍟", "🌭", "🍿", "🧆", "🌮", "🌯", "🫔", "🥙",
        "🧁", "🍩", "🍪", "🎂", "🍰", "🧇", "🥞", "🍫", "🍬", "🍭",
        "☕", "🍵", "🧋", "🥤", "🍺", "🍻", "🥂", "🍷", "🍸", "🍹",
        "🍎", "🍐", "🍊", "🍋", "🍌", "🍉", "🍇", "🍓", "🫐", "🍈",
        "🥑", "🍆", "🥦", "🥬", "🌽", "🌶️", "🫑", "🥒", "🥕", "🧄",
      ],
    },
    bros: {
      label: "🍺",
      emojis: [
        "🍺", "🍻", "🥃", "🍾", "🥴", "🤮", "💀", "☠️", "🖕", "🍑",
        "🍆", "💦", "🥜", "😏", "🤤", "😈", "👿", "🔥", "💩", "🤡",
        "🫣", "🫦", "😵", "🤯", "😤", "🤬", "👀", "🗿", "💪", "🦴",
        "🧠", "🫠", "🤝", "👊", "🤛", "🤜", "🫵", "🤏", "🤌", "✊",
        "🍗", "🌭", "🌮", "🎰", "🚬", "💊", "🧨", "💣", "⚰️", "🪦",
        "🐓", "🐍", "🐒", "🦍", "🐂", "🐎", "🐐", "🫏", "🐖", "🐊",
      ],
    },
    letters: {
      label: "🔤",
      emojis: CUSTOM_LETTERS,
    },
    symbols: {
      label: "💯",
      emojis: [
        "💯", "🔥", "⭐", "🌟", "✨", "⚡", "💥", "💫", "💦", "💨",
        "🕳️", "💬", "💭", "🗯️", "💤", "👁️‍🗨️", "✅", "❌", "❓", "❗",
        "‼️", "⁉️", "💢", "♻️", "🔰", "⚠️", "🚫", "🔞", "📵", "🆘",
        "☢️", "☣️", "⚜️", "🔱", "〽️", "❄️", "🌀", "🎭", "🃏", "🀄",
      ],
    },
  };

  function select(emoji: string) {
    addToHistory(emoji);
    onSelect(emoji);
  }

  function getActiveEmojis(): string[] {
    if (activeCategory === "history") return history;
    return categories[activeCategory].emojis;
  }
</script>

<div class="emoji-picker">
  <div class="categories">
    {#each Object.entries(categories) as [key, cat]}
      <button
        class="cat-btn"
        class:active={activeCategory === key}
        onclick={() => activeCategory = key}
        title={key}
      >
        {cat.label}
      </button>
    {/each}
  </div>
  <div class="emoji-grid">
    {#if activeCategory === "history" && history.length === 0}
      <div class="empty-history">No recent emojis yet</div>
    {:else}
      {#each getActiveEmojis() as emoji}
        {#if isCustomLetter(emoji)}
          <button class="emoji-btn letter-btn" onclick={() => select(emoji)}>
            <span class="letter-square">{getLetterChar(emoji)}</span>
          </button>
        {:else}
          <button class="emoji-btn" onclick={() => select(emoji)}>
            {emoji}
          </button>
        {/if}
      {/each}
    {/if}
  </div>
</div>

<style>
  .emoji-picker {
    position: absolute;
    bottom: 100%;
    left: 0;
    margin-bottom: 8px;
    width: 352px;
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: 10px;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.4);
    overflow: hidden;
    z-index: 100;
  }

  .categories {
    display: flex;
    gap: 2px;
    padding: 8px 8px 4px;
    border-bottom: 1px solid var(--border);
  }

  .cat-btn {
    flex: 1;
    background: none;
    border: none;
    border-radius: 6px;
    padding: 6px;
    font-size: 16px;
    cursor: pointer;
    transition: background-color 0.15s;
  }

  .cat-btn:hover {
    background: var(--bg-input);
  }

  .cat-btn.active {
    background: var(--bg-tertiary);
  }

  .emoji-grid {
    display: grid;
    grid-template-columns: repeat(8, 1fr);
    gap: 2px;
    padding: 8px;
    max-height: 260px;
    overflow-y: auto;
  }

  .emoji-btn {
    background: none;
    border: none;
    border-radius: 6px;
    padding: 4px;
    font-size: 22px;
    cursor: pointer;
    transition: background-color 0.15s;
    line-height: 1;
  }

  .emoji-btn:hover {
    background: var(--bg-input);
  }

  .letter-btn {
    padding: 2px;
    font-size: unset;
  }

  .letter-square {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    background: #5865f2;
    border-radius: 5px;
    color: white;
    font-weight: 700;
    font-size: 16px;
    font-family: 'Segoe UI', sans-serif;
  }

  .letter-btn:hover .letter-square {
    background: #4752c4;
  }

  .empty-history {
    grid-column: 1 / -1;
    text-align: center;
    color: var(--text-muted);
    padding: 24px 0;
    font-size: 13px;
  }
</style>
