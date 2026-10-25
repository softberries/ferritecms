// The palette's Recent group: the last 8 screens and items the viewer opened, newest
// first, per viewer (the signed-in user on this browser) in `localStorage`. Storage can
// be missing or throw (private windows, blocked site data), so every access is guarded
// and falls back to an empty list.

/** How many entries are kept. */
export const RECENT_MAX = 8;

/**
 * A screen is stored by its nav item id (its label and path come from the current nav,
 * so a screen the viewer can no longer see drops out); an item keeps what the palette
 * shows of it, as it was when opened.
 */
export type RecentEntry =
  | { kind: "screen"; id: string }
  | { kind: "item"; type: string; id: string; title: string; status: string };

let viewer: string | null = null;

/** The shell sets the signed-in user's id; `null` stops reading and writing. */
export function setRecentViewer(id: string | null): void {
  viewer = id;
}

export function recentStorageKey(viewerId: string): string {
  return `ferrite.admin.recent.${viewerId}`;
}

/** Identity of an entry: opening it again replaces the older copy. */
export function recentId(entry: RecentEntry): string {
  return entry.kind === "screen" ? `screen:${entry.id}` : `item:${entry.type}:${entry.id}`;
}

const isString = (v: unknown): v is string => typeof v === "string";

function parseEntry(v: unknown): RecentEntry | null {
  if (typeof v !== "object" || v === null) return null;
  const e = v as Record<string, unknown>;
  if (e.kind === "screen" && isString(e.id)) return { kind: "screen", id: e.id };
  if (
    e.kind === "item" &&
    isString(e.type) &&
    isString(e.id) &&
    isString(e.title) &&
    isString(e.status)
  ) {
    return { kind: "item", type: e.type, id: e.id, title: e.title, status: e.status };
  }
  return null;
}

/** The viewer's entries, newest first; empty without a viewer or readable storage. */
export function listRecent(): RecentEntry[] {
  if (viewer === null) return [];
  try {
    const raw = window.localStorage.getItem(recentStorageKey(viewer));
    if (raw === null) return [];
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed.flatMap((v) => parseEntry(v) ?? []).slice(0, RECENT_MAX);
  } catch {
    return [];
  }
}

/** Puts `entry` first (dropping an older copy of it) and keeps the newest 8. */
export function pushRecent(entry: RecentEntry): void {
  if (viewer === null) return;
  const key = recentId(entry);
  const next = [entry, ...listRecent().filter((e) => recentId(e) !== key)].slice(0, RECENT_MAX);
  try {
    window.localStorage.setItem(recentStorageKey(viewer), JSON.stringify(next));
  } catch {
    // Not persisted; nothing else depends on it.
  }
}
