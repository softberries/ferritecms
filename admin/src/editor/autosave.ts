// The autosave slot (07 §3, M2b P15): the working draft of an item, kept per (item, user)
// on the server (`PUT …/autosave`) every 10 s while it changes, so a reload, a crash or
// an expired session never loses what was typed (Review Focus 3). The slot is not a
// revision: the owner's next real save (`PATCH`) deletes it on the server.
//
// `createAutosaver` is the scheduler, free of React and of the API: at most one `save`
// runs at a time, a newer draft waits for the running one, and an explicit save `hold`s
// it, so no `PUT` lands after the `PATCH` that cleared the slot (Task 3 carry). A new,
// never-saved item has no slot (no id yet): the editor keeps its draft in memory and the
// leave guard protects it until the first save creates the item.
//
// `pendingDrafts` keeps a draft across a sign-in: a `401` mid-edit puts the working draft
// here before the router goes to `/login?next=…` (F37), and the editor screen takes it
// back on mount, writes it to the slot and shows the restore banner.

import { NetworkError } from "@/api/client";
import { getAutosave } from "@/api/content";
import { ApiError } from "@/api/problem";
import type { SchemaAutosaveDto } from "@/api/schema";

/** How often a changed draft is written to the slot (07 §3). */
export const AUTOSAVE_INTERVAL_MS = 10_000;

/**
 * An item's working draft as the slot holds it (`AutosaveInput`, pre-flight F11). `null`
 * means "unchanged" for `slug`, `menu_order` and `terms`, "derive from the body" for
 * `excerpt`; `parent_id` is a snapshot (`null` = top level, ruling R4).
 */
export interface Draft {
  /** The item version the draft was made from (its `If-Match` when saved). */
  base_version: number;
  title: string;
  excerpt: string | null;
  body: unknown;
  fields: unknown;
  slug: string | null;
  parent_id: string | null;
  menu_order: number | null;
  terms: string[] | null;
  /** The SEO members (`null` inside: not set); absent or `null`: unchanged. */
  seo?: { title?: string | null; description?: string | null } | null;
}

/** What the slot is doing (the editor folds it into its save indicator). */
export type AutosaveState = "saved" | "dirty" | "saving" | "offline" | "error";

/** A value that changes, with listeners (no dependency on a store library). */
export interface Observable<T> {
  get(): T;
  subscribe(listener: (value: T) => void): () => void;
}

export interface AutosaverOptions {
  /** Writes the draft to the slot; rejects with a `NetworkError` when offline. */
  save: (d: Draft) => Promise<void>;
  intervalMs: number;
  now?: () => number;
  /** A failure that is not a network one (a `401`, a `403`, a `422`): not retried. */
  onError?: (error: unknown) => void;
  /** Where `online` comes from and `visibilitychange` happens (tests pass their own). */
  window?: Pick<Window, "addEventListener" | "removeEventListener">;
  document?: Pick<Document, "addEventListener" | "removeEventListener" | "visibilityState">;
}

export interface Autosaver {
  /** The working draft changed; a function is read when the write starts. */
  markDirty(d: Draft | (() => Draft)): void;
  /**
   * The explicit save (`PATCH`) stored the draft and the server deleted the slot: there
   * is nothing to write any more (until the next `markDirty`).
   */
  markClean(): void;
  /** Writes the pending draft now (after the running write), if there is one. */
  flush(): Promise<void>;
  /**
   * Waits for the running write, then keeps new ones from starting until the returned
   * release is called: an explicit save holds the autosaver around its `PATCH`.
   */
  hold(): Promise<() => void>;
  /** Stops for good: no timer, no listener, no further write. */
  dispose(): void;
  state: Observable<AutosaveState>;
  /** When the slot last took a draft (ms since the epoch), else `null`. */
  savedAt(): number | null;
}

function observable<T>(initial: T): Observable<T> & { set(value: T): void } {
  let value = initial;
  const listeners = new Set<(v: T) => void>();
  return {
    get: () => value,
    set(next) {
      if (Object.is(next, value)) return;
      value = next;
      for (const l of [...listeners]) l(value);
    },
    subscribe(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
}

export function createAutosaver(opts: AutosaverOptions): Autosaver {
  const now = opts.now ?? Date.now;
  const win = opts.window ?? (typeof window === "undefined" ? undefined : window);
  const doc = opts.document ?? (typeof document === "undefined" ? undefined : document);
  const state = observable<AutosaveState>("saved");

  let pending: Draft | (() => Draft) | null = null;
  let running: Promise<void> | null = null;
  let timer: ReturnType<typeof setTimeout> | null = null;
  let holds = 0;
  let disposed = false;
  let lastSaved: number | null = null;

  const arm = () => {
    if (timer !== null || disposed || pending === null) return;
    timer = setTimeout(() => {
      timer = null;
      void write();
    }, opts.intervalMs);
  };

  const settledState = (): AutosaveState => (pending === null ? "saved" : "dirty");

  /** Starts a write of the pending draft unless one runs, a save holds it, or none waits. */
  const write = (): Promise<void> => {
    if (running) return running;
    if (disposed || holds > 0 || pending === null) return Promise.resolve();
    const source = pending;
    pending = null;
    let draft: Draft;
    try {
      draft = typeof source === "function" ? source() : source;
    } catch {
      // The draft can no longer be read (its editor is gone): nothing newer to keep.
      state.set(settledState());
      return Promise.resolve();
    }
    state.set("saving");
    const run = (async () => {
      try {
        await opts.save(draft);
        lastSaved = now();
        if (!disposed) state.set(settledState());
      } catch (e) {
        if (disposed) return;
        if (e instanceof NetworkError) {
          // Not stored: it waits again (unless a newer draft replaced it meanwhile).
          pending ??= source;
          state.set("offline");
        } else {
          // A lost session or a refusal is not retried by itself; a newer draft, or the
          // next edit, tries again. The editor still holds the draft either way.
          state.set("error");
          opts.onError?.(e);
        }
      } finally {
        running = null;
      }
      // A newer draft (or the retry after a network failure) goes at the next tick.
      arm();
    })();
    running = run;
    return run;
  };

  const onOnline = () => {
    if (state.get() === "offline") void write();
  };
  const onVisibility = () => {
    if (doc?.visibilityState === "hidden") void flush();
  };
  win?.addEventListener("online", onOnline);
  doc?.addEventListener("visibilitychange", onVisibility);

  async function flush(): Promise<void> {
    if (running) await running;
    if (pending !== null) await write();
  }

  return {
    markDirty(d) {
      if (disposed) return;
      pending = d;
      if (state.get() !== "saving" && state.get() !== "offline") state.set("dirty");
      arm();
    },
    markClean() {
      pending = null;
      if (timer !== null) clearTimeout(timer);
      timer = null;
      if (!disposed && state.get() !== "saving") state.set("saved");
    },
    flush,
    async hold() {
      holds++;
      if (running) await running.catch(() => undefined);
      let released = false;
      return () => {
        if (released) return;
        released = true;
        holds--;
        if (holds === 0) arm();
      };
    },
    dispose() {
      disposed = true;
      pending = null;
      if (timer !== null) clearTimeout(timer);
      timer = null;
      win?.removeEventListener("online", onOnline);
      doc?.removeEventListener("visibilitychange", onVisibility);
    },
    state,
    savedAt: () => lastSaved,
  };
}

// --- The slot as the screen reads it ---------------------------------------------

/**
 * The caller's slot of the item, or `null` when there is none (`204`) or none it may
 * have (`403`: the screen will be read-only anyway).
 */
export async function loadAutosave(type: string, id: string): Promise<SchemaAutosaveDto | null> {
  try {
    return await getAutosave(type, id);
  } catch (e) {
    if (e instanceof ApiError && e.status === 403) return null;
    throw e;
  }
}

/** The slot as a draft (the server's `undefined` members are `null`: unchanged). */
export function draftOfAutosave(slot: SchemaAutosaveDto): Draft {
  return {
    base_version: slot.base_version,
    title: slot.title,
    excerpt: slot.excerpt ?? null,
    body: slot.body,
    fields: slot.fields,
    slug: slot.slug ?? null,
    parent_id: slot.parent_id ?? null,
    menu_order: slot.menu_order ?? null,
    terms: slot.terms ?? null,
    seo: slot.seo ?? null,
  };
}

// --- Drafts kept across a sign-in (F37) ------------------------------------------

/** A draft waiting for its editor after a `401`, with the account it belongs to. */
export interface PendingDraft {
  userId: string;
  draft: Draft;
  /** When it was taken (RFC 3339): the banner's "from …". */
  at: string;
}

/**
 * Drafts kept in memory across the sign-in a `401` asks for, keyed by item id (or
 * `new:<type>` for an item not created yet). The client's 401 handler leaves it alone;
 * the editor takes its entry on mount (only for the same account) and an explicit
 * sign-out clears it.
 */
export const pendingDrafts = new Map<string, PendingDraft>();

/** The key of a new, not yet created item of `type`. */
export const newDraftKey = (type: string) => `new:${type}`;

/** The waiting draft of `key` for `userId` (another account's is never handed out). */
export function peekPendingDraft(key: string, userId: string | undefined): PendingDraft | null {
  const entry = pendingDrafts.get(key);
  if (!entry || entry.userId !== userId) return null;
  return entry;
}

/** Drops the waiting draft of `key` once the slot (or the item) holds it. */
export function dropPendingDraft(key: string, entry: PendingDraft): void {
  if (pendingDrafts.get(key) === entry) pendingDrafts.delete(key);
}
