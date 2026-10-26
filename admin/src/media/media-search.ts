// The media library's URL state (`/admin/media?view=list&q=…&kind=image&mine=true&trash=true&id=…`)
// and how it maps onto `GET /media` (07 §1). Defaults never reach the URL: the grid
// view, an empty `q`, `mine=false` and `trash=false` parse to "absent"; an invalid
// member is dropped, never an error. `q` is not combinable with `author` (422), so while
// a search runs Mine stays in the URL but is not sent (its chip shows disabled), as in
// the content list. The Trash view has no search (trashed items leave the search index;
// `q` with `trash` is 422): a leftover `q` is not sent there. Without `view` in the URL
// the viewer's last choice applies (`localStorage`), else the grid.

import type { MediaKind } from "@/api/media";
import type { MediaListFilters } from "@/api/keys";
import { storedMediaView, storeMediaView } from "@/design/preferences";
import { z } from "@/lib/zod";

export const MEDIA_KINDS = [
  "image",
  "video",
  "audio",
  "document",
] as const satisfies readonly MediaKind[];

export type MediaView = "grid" | "list";

export interface MediaSearch {
  /** Absent = the remembered view (else `grid`). */
  view?: MediaView;
  q?: string;
  kind?: MediaKind;
  mine?: boolean;
  /** The Trash view (`trash=true`). */
  trash?: boolean;
  /** The item open in the detail sheet. */
  id?: string;
}

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const flag = z
  .union([z.boolean(), z.literal("true"), z.literal("false")])
  .transform((v) => v === true || v === "true")
  .optional()
  .catch(undefined);

// The router JSON-decodes each value but free text (`lib/search-params.ts`): `q` is the
// text as typed, `mine=true` a boolean (the string spelling is accepted too).
const schema = z.object({
  view: z.enum(["grid", "list"]).optional().catch(undefined),
  q: z
    .string()
    .transform((v) => v.trim())
    .optional()
    .catch(undefined),
  kind: z.enum(MEDIA_KINDS).optional().catch(undefined),
  mine: flag,
  trash: flag,
  id: z.string().regex(UUID).optional().catch(undefined),
});

/** `validateSearch` of the route: valid members only, defaults omitted. */
export function parseMediaSearch(raw: Record<string, unknown>): MediaSearch {
  const parsed = schema.parse(raw);
  const out: MediaSearch = {};
  if (parsed.view === "list") out.view = "list";
  if (parsed.q) out.q = parsed.q;
  if (parsed.kind !== undefined) out.kind = parsed.kind;
  if (parsed.mine === true) out.mine = true;
  if (parsed.trash === true) out.trash = true;
  if (parsed.id !== undefined) out.id = parsed.id.toLowerCase();
  return out;
}

/** Whether a search is running, so the Mine chip is inactive (07 §1: `q` excludes it). */
export function searchDisables(search: MediaSearch): boolean {
  return search.q !== undefined && search.trash !== true;
}

/** Whether `search` narrows the library (for the empty state and "Clear filters"). */
export function hasFilters(search: MediaSearch): boolean {
  return (
    (search.q !== undefined && search.trash !== true) ||
    search.kind !== undefined ||
    search.mine === true
  );
}

/**
 * The list request for `search` (without `cursor`; pages of 50, the server default):
 * `mine` becomes `author=<me>`, a search drops `author`, the trash drops `q`.
 */
export function mediaQuery(search: MediaSearch, meId: string): MediaListFilters {
  const query: MediaListFilters = {};
  if (search.trash === true) query.trash = true;
  else if (search.q !== undefined) query.q = search.q;
  if (search.mine === true && query.q === undefined) query.author = meId;
  if (search.kind !== undefined) query.kind = search.kind;
  return query;
}

/** The view to show: the URL's, else the viewer's last choice, else the grid. */
export function effectiveView(search: MediaSearch): MediaView {
  return search.view ?? storedMediaView();
}

/** Remembers the viewer's choice for the next visit without `view` in the URL. */
export function rememberView(view: MediaView): void {
  storeMediaView(view);
}
