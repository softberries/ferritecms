// The comments queue's URL state (`/admin/comments?status=spam&item=<uuid>`) and how it
// maps onto `GET /comments` (07 §1). The queue opens on Pending, so `status=pending` is
// the absent default; `item` narrows the queue (and the tab counts) to one content item.
// An invalid member is dropped, never an error: a hand-edited or stale link still opens
// the queue.

import type { CommentFilters } from "@/api/keys";
import { z } from "@/lib/zod";

export const COMMENT_STATUSES = ["pending", "approved", "spam", "trash"] as const;
export type CommentStatus = (typeof COMMENT_STATUSES)[number];

/** Comments per page of the queue (Global Constraints: lists page by 50). */
export const COMMENT_PAGE_SIZE = 50;

export interface CommentSearch {
  /** Absent = `pending`. */
  status?: Exclude<CommentStatus, "pending">;
  /** A content item id (uuid, lower case). */
  item?: string;
}

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

const schema = z.object({
  status: z.enum(COMMENT_STATUSES).optional().catch(undefined),
  item: z.string().regex(UUID).optional().catch(undefined),
});

/** `validateSearch` of the route: valid members only, the default omitted. */
export function parseCommentSearch(raw: Record<string, unknown>): CommentSearch {
  const parsed = schema.parse(raw);
  const out: CommentSearch = {};
  if (parsed.status !== undefined && parsed.status !== "pending") out.status = parsed.status;
  if (parsed.item !== undefined) out.item = parsed.item.toLowerCase();
  return out;
}

/** The status `search` shows. */
export function statusOfSearch(search: CommentSearch): CommentStatus {
  return search.status ?? "pending";
}

/** The list request for `search` (without `cursor`). */
export function commentQuery(search: CommentSearch): CommentFilters & { limit: number } {
  return {
    status: statusOfSearch(search),
    ...(search.item !== undefined ? { item: search.item } : {}),
    limit: COMMENT_PAGE_SIZE,
  };
}

/** `search` on another tab: the item filter stays; `pending` is the absent default. */
export function withStatus(search: CommentSearch, status: CommentStatus): CommentSearch {
  const next: CommentSearch = { ...search };
  delete next.status;
  return status === "pending" ? next : { ...next, status };
}
