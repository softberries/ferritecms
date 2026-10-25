// Word-level diffs for the conflict dialog (Task 16) and the revisions drawer (Task 17):
// `diffWordsWithSpace` from `diff` 9.0.0, so whitespace changes show too. A body is
// compared through its plain text (`blockText`, the same text the site indexes).

import { diffWordsWithSpace } from "diff";
import { blockText } from "./text";

export interface DiffPart {
  kind: "same" | "added" | "removed";
  text: string;
}

/** The parts that turn `a` into `b`, adjacent parts of one kind merged. */
export function diffWords(a: string, b: string): DiffPart[] {
  const parts: DiffPart[] = [];
  for (const change of diffWordsWithSpace(a, b)) {
    if (change.value === "") continue;
    const kind = change.added ? "added" : change.removed ? "removed" : "same";
    const last = parts.at(-1);
    if (last?.kind === kind) last.text += change.value;
    else parts.push({ kind, text: change.value });
  }
  return parts;
}

export interface RevisionSide {
  title: string;
  excerpt: string;
  body: unknown;
}

export function diffRevision(
  older: RevisionSide,
  newer: RevisionSide,
): { title: DiffPart[]; excerpt: DiffPart[]; body: DiffPart[] } {
  return {
    title: diffWords(older.title, newer.title),
    excerpt: diffWords(older.excerpt, newer.excerpt),
    body: diffWords(blockText(older.body), blockText(newer.body)),
  };
}
