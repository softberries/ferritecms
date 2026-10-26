// The upload queue of one browser tab (Global Constraints: at most 3 uploads at once;
// the server's 8 per node are shared by every editor). Each file is an entry with an
// `Idempotency-Key` made once for the entry's life, so every retry of it (automatic or
// by **Retry**) is the same upload to the server: a response lost on the way back
// cannot create a second item (Review Focus 1). A `503 busy` or a network failure is
// retried once by itself after `Retry-After` (else 2 s), without holding a slot while it
// waits. A `409 conflict` (the same key's earlier request, cut off by a dropped
// connection, still runs on the server) is waited out the same way, apart from that one
// retry and at most `IN_PROGRESS_WAITS` times in a row, so the retry after a network
// failure still gets the server's answer for the key; any other failure waits for
// **Retry**. `cancel` aborts a running upload or drops
// a queued one. Framework-free: the tray reads it through `subscribe` + `entries`.

import type { MediaDto } from "@/api/media";
import { ApiError } from "@/api/problem";
import { UploadNetworkError, type uploadMedia } from "@/api/upload";

/** How long an automatic retry waits when the server sent no `Retry-After`. */
export const BUSY_RETRY_DEFAULT_MS = 2_000;

/** How many `409 conflict` answers in a row are waited out before the row fails. */
export const IN_PROGRESS_WAITS = 6;

export type UploadState = "queued" | "uploading" | "done" | "failed" | "cancelled";

export interface UploadEntry {
  id: string;
  file: File;
  /** `Idempotency-Key`: `crypto.randomUUID()`, fixed for the entry's life. */
  key: string;
  state: UploadState;
  loaded: number;
  total: number;
  error?: ApiError | Error;
  media?: MediaDto;
  /** Queued again after a `503 busy`, a `409 conflict` or a network failure, until its wait ends. */
  waiting?: boolean;
}

export interface UploadQueueOptions {
  upload: typeof uploadMedia;
  concurrency: number;
  onDone(media: MediaDto): void;
}

export interface UploadQueue {
  add(files: File[]): void;
  retry(id: string): void;
  /** Aborts a running upload, drops a queued one, dismisses a failed one. */
  cancel(id: string): void;
  /** Removes the finished rows (done and cancelled). */
  clearDone(): void;
  /** Calls `fn` after every change; returns the unsubscribe. */
  subscribe(fn: () => void): () => void;
  /** The rows, oldest first; a new array after every change (for `useSyncExternalStore`). */
  entries(): readonly UploadEntry[];
  /** Whether anything is queued or uploading (a waiting retry included). */
  active(): boolean;
  /** Cancels everything (the screen is left). */
  dispose(): void;
}

/** Whether the key's earlier request still runs on the server (`409 conflict`). */
function inProgress(error: unknown): boolean {
  return error instanceof ApiError && error.status === 409 && error.slug === "conflict";
}

/** Whether a failure is retried once by itself. */
function transient(error: unknown): boolean {
  return (
    error instanceof UploadNetworkError ||
    (error instanceof ApiError && error.status === 503 && error.slug === "busy")
  );
}

function withoutError(row: UploadEntry): UploadEntry {
  const copy = { ...row };
  delete copy.error;
  return copy;
}

function isAbort(error: unknown): boolean {
  return error instanceof DOMException && error.name === "AbortError";
}

interface Runtime {
  controller?: AbortController;
  timer?: ReturnType<typeof setTimeout>;
  /** Automatic retries used (at most one per failure streak). */
  autoRetried: boolean;
  /** `409 conflict` answers waited out in a row ([`IN_PROGRESS_WAITS`]). */
  inProgressWaits: number;
}

export function createUploadQueue(opts: UploadQueueOptions): UploadQueue {
  let rows: UploadEntry[] = [];
  const runtime = new Map<string, Runtime>();
  const listeners = new Set<() => void>();
  let seq = 0;

  const emit = () => {
    rows = [...rows];
    for (const fn of listeners) fn();
  };
  const patch = (id: string, change: Partial<UploadEntry>) => {
    rows = rows.map((r) => (r.id === id ? { ...r, ...change } : r));
  };
  const find = (id: string) => rows.find((r) => r.id === id);

  const pump = () => {
    let running = rows.filter((r) => r.state === "uploading").length;
    for (const row of rows) {
      if (running >= opts.concurrency) break;
      if (row.state !== "queued" || row.waiting) continue;
      running += 1;
      start(row);
    }
  };

  const start = (row: UploadEntry) => {
    const rt = runtime.get(row.id);
    if (!rt) return;
    const controller = new AbortController();
    rt.controller = controller;
    rows = rows.map((r) =>
      r.id === row.id
        ? { ...withoutError(r), state: "uploading", loaded: 0, total: row.file.size }
        : r,
    );
    opts
      .upload(row.file, {
        key: row.key,
        onProgress: (loaded, total) => {
          if (find(row.id)?.state !== "uploading") return;
          patch(row.id, { loaded, total });
          emit();
        },
        signal: controller.signal,
      })
      .then(
        (media) => {
          if (find(row.id)?.state !== "uploading") return;
          rt.autoRetried = false;
          rt.inProgressWaits = 0;
          const total = find(row.id)?.total ?? row.file.size;
          patch(row.id, { state: "done", media, loaded: total });
          opts.onDone(media);
          pump();
          emit();
        },
        (error: unknown) => {
          // A cancel already settled the row.
          if (find(row.id)?.state !== "uploading" || isAbort(error)) return;
          const err = error instanceof Error ? error : new Error(String(error));
          const waitOut = inProgress(error)
            ? rt.inProgressWaits < IN_PROGRESS_WAITS
            : transient(error) && !rt.autoRetried;
          if (waitOut) {
            if (inProgress(error)) rt.inProgressWaits += 1;
            else rt.autoRetried = true;
            const wait =
              error instanceof ApiError && error.retryAfter !== null
                ? error.retryAfter * 1000
                : BUSY_RETRY_DEFAULT_MS;
            patch(row.id, { state: "queued", waiting: true, error: err, loaded: 0 });
            rt.timer = setTimeout(() => {
              rt.timer = undefined;
              if (find(row.id)?.state !== "queued") return;
              patch(row.id, { waiting: false });
              pump();
              emit();
            }, wait);
          } else {
            rt.autoRetried = false;
            rt.inProgressWaits = 0;
            patch(row.id, { state: "failed", error: err });
          }
          pump();
          emit();
        },
      );
  };

  return {
    add(files) {
      for (const file of files) {
        seq += 1;
        const id = `upload-${seq}`;
        runtime.set(id, { autoRetried: false, inProgressWaits: 0 });
        rows.push({
          id,
          file,
          key: crypto.randomUUID(),
          state: "queued",
          loaded: 0,
          total: file.size,
        });
      }
      pump();
      emit();
    },
    retry(id) {
      const row = find(id);
      if (!row || (row.state !== "failed" && row.state !== "cancelled")) return;
      rows = rows.map((r) =>
        r.id === id ? { ...withoutError(r), state: "queued", loaded: 0 } : r,
      );
      pump();
      emit();
    },
    cancel(id) {
      const row = find(id);
      if (!row || (row.state !== "queued" && row.state !== "uploading" && row.state !== "failed")) {
        return;
      }
      const rt = runtime.get(id);
      patch(id, { state: "cancelled", waiting: false });
      if (rt?.timer !== undefined) {
        clearTimeout(rt.timer);
        rt.timer = undefined;
      }
      rt?.controller?.abort();
      pump();
      emit();
    },
    clearDone() {
      const gone = rows.filter((r) => r.state === "done" || r.state === "cancelled");
      for (const r of gone) runtime.delete(r.id);
      rows = rows.filter((r) => r.state !== "done" && r.state !== "cancelled");
      emit();
    },
    subscribe(fn) {
      listeners.add(fn);
      return () => {
        listeners.delete(fn);
      };
    },
    entries: () => rows,
    active: () => rows.some((r) => r.state === "queued" || r.state === "uploading"),
    dispose() {
      for (const row of rows) {
        if (row.state !== "queued" && row.state !== "uploading") continue;
        const rt = runtime.get(row.id);
        if (rt?.timer !== undefined) clearTimeout(rt.timer);
        rt?.controller?.abort();
        patch(row.id, { state: "cancelled", waiting: false });
      }
      emit();
    },
  };
}
