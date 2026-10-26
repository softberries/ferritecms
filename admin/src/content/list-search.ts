// The content list's URL state (`/admin/content/$type?status=…&q=…&mine=true&term=…&order=…`)
// and how it maps onto `GET /content/{type}` (07 §1). Defaults are never written to the
// URL: `status=all`, `order=updated`, `mine=false` and an empty `q` parse to "absent".
// An invalid member is dropped, never an error: a hand-edited or stale link still opens
// the list. `q` is not combinable with `author`, `term` or `parent` on the server (422),
// so while a search is active the Mine and term filters stay in the URL but are not sent
// (the chips show them disabled); `order` is ignored by the server for a search
// (results come by relevance), so it is not sent either.

import { CONTENT_STATUSES, type ContentStatus, type ContentListQuery } from "@/api/content";
import type { SchemaContentCountsDto } from "@/api/schema";
import { z } from "@/lib/zod";

export const LIST_ORDERS = ["updated", "published", "title"] as const;
export type ListOrder = (typeof LIST_ORDERS)[number];

/** Page size of the list (Global Constraints: `limit=50`). */
export const LIST_PAGE_SIZE = 50;

/** How long the search field waits after the last keystroke before it updates the URL. */
export const SEARCH_DEBOUNCE_MS = 200;

export interface ListSearch {
  /** Absent = "all": every status but trash. */
  status?: ContentStatus;
  q?: string;
  mine?: boolean;
  /** A term id (uuid). */
  term?: string;
  /** Absent = `updated`. */
  order?: Exclude<ListOrder, "updated">;
}

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

// The router JSON-decodes each value but free text (`lib/search-params.ts`): `q` is the
// text as typed, `mine=true` a boolean (the string spelling is accepted too).
const schema = z.object({
  status: z
    .enum([...CONTENT_STATUSES, "all"])
    .optional()
    .catch(undefined),
  q: z
    .string()
    .transform((v) => v.trim())
    .optional()
    .catch(undefined),
  mine: z
    .union([z.boolean(), z.literal("true"), z.literal("false")])
    .transform((v) => v === true || v === "true")
    .optional()
    .catch(undefined),
  term: z.string().regex(UUID).optional().catch(undefined),
  order: z.enum(LIST_ORDERS).optional().catch(undefined),
});

/** `validateSearch` of the route: valid members only, defaults omitted. */
export function parseListSearch(raw: Record<string, unknown>): ListSearch {
  const parsed = schema.parse(raw);
  const out: ListSearch = {};
  if (parsed.status !== undefined && parsed.status !== "all") out.status = parsed.status;
  if (parsed.q) out.q = parsed.q;
  if (parsed.mine === true) out.mine = true;
  if (parsed.term !== undefined) out.term = parsed.term.toLowerCase();
  if (parsed.order !== undefined && parsed.order !== "updated") out.order = parsed.order;
  return out;
}

/** Whether `search` narrows the list beyond its status tab (for the empty state). */
export function hasFilters(search: ListSearch): boolean {
  return search.q !== undefined || search.mine === true || search.term !== undefined;
}

/** Whether a filter chip is inactive because a search is running (07 §1: `q` excludes them). */
export function searchDisables(search: ListSearch): boolean {
  return search.q !== undefined;
}

/**
 * The list request for `search` (without `cursor`): `mine` becomes `author=<me>` (a
 * UUID, pre-flight F34), and a search drops `author`, `term` and `order`.
 */
export function listQuery(search: ListSearch, meId: string): Omit<ContentListQuery, "cursor"> {
  const query: Omit<ContentListQuery, "cursor"> = { limit: LIST_PAGE_SIZE };
  if (search.status !== undefined) query.status = search.status;
  if (search.q !== undefined) {
    query.q = search.q;
    return query;
  }
  if (search.mine === true) query.author = meId;
  if (search.term !== undefined) query.term = search.term;
  query.order = search.order ?? "updated";
  return query;
}

export const STATUS_TABS = [
  "all",
  "draft",
  "pending",
  "scheduled",
  "published",
  "private",
  "trash",
] as const;
export type StatusTab = (typeof STATUS_TABS)[number];

/** The tab's count: "All" is every status but trash (the list's default). */
export function tabCount(counts: SchemaContentCountsDto, tab: StatusTab): number {
  if (tab !== "all") return counts[tab];
  return counts.draft + counts.pending + counts.scheduled + counts.published + counts.private;
}
