// Word count and reading time for the editor footer. Words are what `Intl.Segmenter`
// calls word-like (so CJK text counts per word, not per run), else runs of letters and
// digits.

const segmenter =
  typeof Intl !== "undefined" && "Segmenter" in Intl
    ? new Intl.Segmenter(undefined, { granularity: "word" })
    : null;

export function wordCount(text: string): number {
  if (text.trim() === "") return 0;
  if (segmenter) {
    let n = 0;
    for (const s of segmenter.segment(text)) if (s.isWordLike) n++;
    return n;
  }
  return text.match(/[\p{L}\p{N}]+(?:['’][\p{L}\p{N}]+)*/gu)?.length ?? 0;
}

/** Words per minute of an adult reading silently (the usual 238 wpm estimate). */
export const WORDS_PER_MINUTE = 238;

/** Whole minutes to read `words`, at least 1 for any text, 0 for none. */
export function readingMinutes(words: number): number {
  return words === 0 ? 0 : Math.max(1, Math.round(words / WORDS_PER_MINUTE));
}
