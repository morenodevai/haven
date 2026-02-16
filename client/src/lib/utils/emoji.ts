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
