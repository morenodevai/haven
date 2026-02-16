// Custom letter emoji format: [:X:] where X is A-Z or 0-9
const CUSTOM_LETTER_RE = /^\[:.\:]$/;
const CUSTOM_LETTER_GLOBAL_RE = /\[:.\:]/g;

export function isCustomLetter(emoji: string): boolean {
  return CUSTOM_LETTER_RE.test(emoji);
}

export function getLetterChar(emoji: string): string {
  return emoji.charAt(2);
}

// Check if a message text contains any custom letter tokens
export function hasCustomLetters(text: string): boolean {
  return CUSTOM_LETTER_GLOBAL_RE.test(text);
}

// Check if a string is a multi-letter word reaction (all [:X:] tokens, more than one)
const LETTER_WORD_RE = /^(\[:.\:]){2,}$/;
export function isLetterWord(emoji: string): boolean {
  return LETTER_WORD_RE.test(emoji);
}

// Split a word reaction like "[:H:][:E:][:L:][:L:][:O:]" into ["H","E","L","L","O"]
export function parseLetters(emoji: string): string[] {
  const matches = emoji.match(CUSTOM_LETTER_GLOBAL_RE);
  return matches ? matches.map(m => m.charAt(2)) : [];
}
