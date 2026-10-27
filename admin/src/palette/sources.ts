// The palette's live content search (docs/design/11-admin-ui.md §7, plan Task 11): one
// `GET /content/{type}?q=…&limit=8` per type, in parallel, after a 150 ms debounce. The
// caller aborts the previous round on every keystroke (TanStack Query does it when the
// query key changes), so a round that is still waiting sends nothing.

import { listContentOfType } from "@/api/content";

/** How long a round waits before it sends anything (constraints: exact values). */
export const SEARCH_DEBOUNCE_MS = 150;
/** Shorter queries (after trimming) are not sent. */
export const SEARCH_MIN_CHARS = 2;
/** Hits per type. */
export const SEARCH_LIMIT = 8;
/** Characters of the excerpt a hit matched only in its text shows. */
export const SNIPPET_CHARS = 120;

/**
 * One content hit. Summaries carry no permalink (pre-flight F6), so the palette shows
 * the slug.
 */
export interface PaletteHit {
  type: string;
  id: string;
  title: string;
  status: string;
  slug: string;
  /** Plain text (the item's excerpt); shown for a hit its title does not explain. */
  excerpt: string;
}

/** The words of `query` the palette marks and matches: 2+ characters, regex-escaped. */
export function queryWords(query: string): string[] {
  return query
    .split(/\s+/)
    .filter((w) => Array.from(w).length >= 2)
    .map((w) => w.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"));
}

/**
 * For a hit whose title holds none of the query's words (the server matched its text):
 * the first `SNIPPET_CHARS` characters of its excerpt, as plain text ("" when it has
 * none). `null` when the title explains the hit, or the query has no word to judge by.
 */
export function bodySnippet(
  hit: Pick<PaletteHit, "title" | "excerpt">,
  query: string,
): string | null {
  const words = queryWords(query);
  if (words.length === 0) return null;
  if (new RegExp(words.join("|"), "i").test(hit.title)) return null;
  const chars = Array.from(hit.excerpt.replace(/\s+/g, " ").trim());
  return chars.length > SNIPPET_CHARS
    ? `${chars.slice(0, SNIPPET_CHARS).join("").trimEnd()}…`
    : chars.join("");
}

/** Whether `q` is long enough to search for. */
export function searchable(q: string): boolean {
  return Array.from(q.trim()).length >= SEARCH_MIN_CHARS;
}

function abortError(signal: AbortSignal): DOMException {
  return signal.reason instanceof DOMException && signal.reason.name === "AbortError"
    ? signal.reason
    : new DOMException("The search was aborted.", "AbortError");
}

/** Resolves after `ms`, or rejects with an `AbortError` as soon as `signal` aborts. */
function wait(ms: number, signal: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal.aborted) {
      reject(abortError(signal));
      return;
    }
    const timer = setTimeout(() => {
      signal.removeEventListener("abort", onAbort);
      resolve();
    }, ms);
    const onAbort = () => {
      clearTimeout(timer);
      reject(abortError(signal));
    };
    signal.addEventListener("abort", onAbort, { once: true });
  });
}

/**
 * Searches every type in `types` for `q`. Hits come in `types` order, and each type's
 * hits in the server's order (relevance). A type deleted meanwhile has no hits and calls
 * `onGone`; a type that fails is left out; if every type fails, the first error is thrown. An abort rejects with an `AbortError`.
 */
export async function searchContent(
  q: string,
  types: string[],
  signal: AbortSignal,
  onGone: () => void = () => undefined,
): Promise<PaletteHit[]> {
  const query = q.trim();
  if (!searchable(query) || types.length === 0) return [];
  await wait(SEARCH_DEBOUNCE_MS, signal);
  const rounds = await Promise.allSettled(
    types.map((type) =>
      listContentOfType(type, { q: query, limit: SEARCH_LIMIT }, { signal, onGone }),
    ),
  );
  if (signal.aborted) throw abortError(signal);

  const hits: PaletteHit[] = [];
  let firstError: unknown = null;
  let failures = 0;
  for (const round of rounds) {
    if (round.status === "rejected") {
      failures += 1;
      firstError ??= round.reason;
      continue;
    }
    for (const item of round.value.items) {
      hits.push({
        type: item.type,
        id: item.id,
        title: item.title,
        status: item.status,
        slug: item.slug,
        excerpt: item.excerpt,
      });
    }
  }
  if (failures === rounds.length) throw firstError;
  return hits;
}
