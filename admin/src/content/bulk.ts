// Bulk actions on content (M2b P20) and the undo of a trash (P21, pre-flight F36).
//
// Bulk is a client-side fan-out: one request per item, at most `concurrency` at a time,
// each with the item's own `If-Match`, and only for items whose `allowed[action]` is true
// (the server decides; the list only hides). Failures are collected with their problem
// document, so a 412 on one item leaves the others done and is listed on its own.
//
// Undo after trash: `restore` always yields a draft, so undoing the trash of a published,
// scheduled or pending item re-applies its previous status when the restored item's
// `allowed` permits it: published → publish (keeps the first `published_at`), scheduled →
// schedule at the old time if it is still ahead, pending → submit. Private cannot be
// re-applied (no transition reaches it), and a lapsed schedule cannot either; both come
// back as drafts with a notice.

import type { ContentStatus } from "@/api/content";
import { ApiError } from "@/api/problem";
import type { SchemaContentDto, SchemaContentSummaryDto } from "@/api/schema";

export type BulkAction = "trash" | "restore" | "publish" | "delete";

/** What a bulk action needs from an item (a `ContentSummaryDto` or a `ContentDto`). */
export type BulkItem = Pick<SchemaContentSummaryDto, "id" | "allowed">;

export interface BulkFailure {
  id: string;
  error: ApiError;
}

export interface BulkResult {
  /** Ids done, in the order of `items`. */
  ok: string[];
  /** Ids that failed, in the order of `items`, each with its problem. */
  failed: BulkFailure[];
}

/** How many requests a bulk action keeps in flight (P20). */
export const BULK_CONCURRENCY = 4;

/** A failure as an `ApiError`: a request that never got an answer has status 0. */
export function asApiError(error: unknown): ApiError {
  if (error instanceof ApiError) return error;
  return new ApiError({
    type: "about:blank",
    title: error instanceof Error ? error.name : "Error",
    status: 0,
    detail: "",
  });
}

/**
 * Runs `run` for every item `allowed[action]` permits, `concurrency` at a time; the
 * others are skipped (in neither list).
 */
export async function runBulk<T extends BulkItem>(
  items: readonly T[],
  action: BulkAction,
  run: (item: T) => Promise<unknown>,
  concurrency: number = BULK_CONCURRENCY,
): Promise<BulkResult> {
  const eligible = items.filter((item) => item.allowed[action]);
  const outcome: ({ ok: true } | { ok: false; error: ApiError } | undefined)[] = [];
  let next = 0;
  const worker = async () => {
    while (next < eligible.length) {
      const index = next++;
      const item = eligible[index];
      if (item === undefined) return;
      try {
        await run(item);
        outcome[index] = { ok: true };
      } catch (e) {
        outcome[index] = { ok: false, error: asApiError(e) };
      }
    }
  };
  await Promise.all(
    Array.from({ length: Math.max(1, Math.min(concurrency, eligible.length)) }, worker),
  );
  const result: BulkResult = { ok: [], failed: [] };
  eligible.forEach((item, i) => {
    const o = outcome[i];
    if (o?.ok) result.ok.push(item.id);
    else if (o) result.failed.push({ id: item.id, error: o.error });
  });
  return result;
}

/** How many of `items` an action applies to (the bulk bar's per-action count). */
export function countAllowed(items: readonly BulkItem[], action: BulkAction): number {
  return items.reduce((n, item) => n + (item.allowed[action] ? 1 : 0), 0);
}

/** An item as it was before the trash, and the trashed item the server returned. */
export interface Trashed {
  before: Pick<SchemaContentSummaryDto, "id" | "status" | "scheduled_at">;
  after: SchemaContentDto;
}

/** The lifecycle calls an undo needs (`lifecycle` from `@/api/content`, bound to a type). */
export interface UndoTransitions {
  restore: (id: string, version: number) => Promise<SchemaContentDto>;
  publish: (id: string, version: number) => Promise<SchemaContentDto>;
  submit: (id: string, version: number) => Promise<SchemaContentDto>;
  schedule: (id: string, version: number, at: string) => Promise<SchemaContentDto>;
}

/**
 * Why an undone item is not back in its old status: `scheduleLapsed` (the time has
 * passed), `privateLost` (no transition reaches private), `notAllowed` (the restored
 * item's `allowed` does not permit it), `failed` (the re-apply request failed).
 */
export type UndoNotice = "scheduleLapsed" | "privateLost" | "notAllowed" | "failed";

export interface Undone {
  item: SchemaContentDto;
  notice?: UndoNotice;
}

/** Restores a trashed item and re-applies its previous status (F36). */
export async function undoTrash(
  entry: Trashed,
  ops: UndoTransitions,
  now: number = Date.now(),
): Promise<Undone> {
  const restored = await ops.restore(entry.after.id, entry.after.version);
  const previous = entry.before.status as ContentStatus;
  const reapply = async (
    allowed: boolean,
    call: () => Promise<SchemaContentDto>,
  ): Promise<Undone> => {
    if (!allowed) return { item: restored, notice: "notAllowed" };
    try {
      return { item: await call() };
    } catch {
      return { item: restored, notice: "failed" };
    }
  };
  switch (previous) {
    case "published":
      return reapply(restored.allowed.publish, () => ops.publish(restored.id, restored.version));
    case "pending":
      return reapply(restored.allowed.submit, () => ops.submit(restored.id, restored.version));
    case "scheduled": {
      const at = entry.before.scheduled_at;
      if (!at || Date.parse(at) <= now) return { item: restored, notice: "scheduleLapsed" };
      return reapply(restored.allowed.schedule, () =>
        ops.schedule(restored.id, restored.version, at),
      );
    }
    case "private":
      return { item: restored, notice: "privateLost" };
    default:
      return { item: restored };
  }
}
