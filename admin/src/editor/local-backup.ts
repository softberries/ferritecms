// The local backup of a working draft (M2b-2 Task 14; ruling R13, plan P40). After Take
// theirs the user's own version is kept in the autosave slot (one per item and user), so
// new edits cannot go there; until they are saved or discarded they live only in the
// page. This keeps them in `localStorage` too, under `ferrite:backup:<item>:<user>`, at
// most once a second, so a crash or a closed tab does not lose them: on the next load
// the restore banner offers them ("Unsaved edits from this browser"). The editor writes
// it only while a kept copy holds the slot and the working draft has unsaved edits, and
// clears it on a successful save, on Discard and on sign-out.
//
// Storage can be missing or full (a private window, a quota): every access is guarded,
// and a failed write is a `console.warn`, never an error: the page still holds the edits
// and the leave guard still asks.

import type { Draft } from "./autosave";

/** The least time between two writes of one backup (Global Constraints: once a second). */
export const BACKUP_INTERVAL_MS = 1000;

const PREFIX = "ferrite:backup:";

/** Whose backup of which item. */
export interface BackupKey {
  item: string;
  user: string;
}

/** A backup as read back: the draft and when it was written (RFC 3339). */
export interface Backup {
  draft: Draft;
  saved_at: string;
}

const storageKey = (key: BackupKey) => `${PREFIX}${key.item}:${key.user}`;

interface Throttle {
  /** When the last write went (ms since the epoch). */
  last: number;
  /** The newest draft waiting for the end of the interval. */
  waiting: Draft | null;
  timer: ReturnType<typeof setTimeout> | null;
}

const throttles = new Map<string, Throttle>();

function store(key: string, draft: Draft): void {
  try {
    const backup: Backup = { draft, saved_at: new Date().toISOString() };
    window.localStorage.setItem(key, JSON.stringify(backup));
  } catch (e) {
    // Full or unavailable: the page still holds the edits (and the leave guard asks).
    console.warn("The local backup of the draft could not be written.", e);
  }
}

function forget(key: string): void {
  const t = throttles.get(key);
  if (t?.timer) clearTimeout(t.timer);
  throttles.delete(key);
}

/**
 * Writes `draft` as the backup of `key`: at once when the last write is a second old or
 * more, else the newest draft goes when the second is over.
 */
export function writeBackup(key: BackupKey, draft: Draft): void {
  const k = storageKey(key);
  const now = Date.now();
  const t = throttles.get(k);
  if (!t || now - t.last >= BACKUP_INTERVAL_MS) {
    if (t?.timer) clearTimeout(t.timer);
    throttles.set(k, { last: now, waiting: null, timer: null });
    store(k, draft);
    return;
  }
  t.waiting = draft;
  t.timer ??= setTimeout(
    () => {
      t.timer = null;
      const next = t.waiting;
      t.waiting = null;
      if (next === null) return;
      t.last = Date.now();
      store(k, next);
    },
    t.last + BACKUP_INTERVAL_MS - now,
  );
}

const isObject = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);

/** The backup of `key`, or `null` (none, unreadable, or not a backup). */
export function readBackup(key: BackupKey): Backup | null {
  let raw: string | null;
  try {
    raw = window.localStorage.getItem(storageKey(key));
  } catch {
    return null;
  }
  if (raw === null) return null;
  try {
    const parsed: unknown = JSON.parse(raw);
    if (!isObject(parsed) || typeof parsed.saved_at !== "string") return null;
    const draft = parsed.draft;
    if (
      !isObject(draft) ||
      typeof draft.title !== "string" ||
      typeof draft.base_version !== "number"
    ) {
      return null;
    }
    return { draft: draft as unknown as Draft, saved_at: parsed.saved_at };
  } catch {
    return null;
  }
}

/** Removes the backup of `key` (and a write still waiting for it). */
export function clearBackup(key: BackupKey): void {
  const k = storageKey(key);
  forget(k);
  try {
    window.localStorage.removeItem(k);
  } catch {
    // Unavailable storage holds nothing to remove.
  }
}

/** Removes every backup of `user` (sign-out). */
export function clearAllBackups(user: string): void {
  const suffix = `:${user}`;
  for (const k of [...throttles.keys()]) if (k.endsWith(suffix)) forget(k);
  try {
    const storage = window.localStorage;
    const keys: string[] = [];
    for (let i = 0; i < storage.length; i++) {
      const k = storage.key(i);
      if (k?.startsWith(PREFIX) && k.endsWith(suffix)) keys.push(k);
    }
    for (const k of keys) storage.removeItem(k);
  } catch {
    // Unavailable storage holds nothing to remove.
  }
}
