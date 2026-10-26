// What an item's parent is to the viewer: one resolver shared by the content list's
// parent cell and the editor's parent field. A parent the viewer may not read (someone
// else's draft above a published child: the server allows it) answers 404 (or 403);
// that is a settled answer, "not available", never retried and never shown as loading
// forever. The child keeps its `parent_id` either way: nothing here changes it.

import { queryOptions } from "@tanstack/react-query";
import { getContent } from "@/api/content";
import { qk } from "@/api/keys";
import { ApiError } from "@/api/problem";

/** A parent's title and slug, as far as the viewer may see them. */
export interface ParentRef {
  title: string;
  slug: string;
}

/** `GET /content/{type}/{id}` as a parent: the reference, or `null` when not readable. */
export async function fetchParentRef(type: string, id: string): Promise<ParentRef | null> {
  try {
    const item = await getContent(type, id);
    return { title: item.title, slug: item.slug };
  } catch (e) {
    if (e instanceof ApiError && (e.status === 404 || e.status === 403)) return null;
    throw e;
  }
}

/** The shared query (`null` data: settled, not available to this viewer). */
export function parentRefQuery(type: string, id: string) {
  return queryOptions({
    queryKey: qk.content.parentRef(type, id),
    queryFn: () => fetchParentRef(type, id),
    staleTime: 60_000,
  });
}

export type ParentState =
  | { kind: "none" }
  | { kind: "loading" }
  | { kind: "found"; ref: ParentRef }
  /** Not readable by this viewer (404/403), or the lookup failed for good. */
  | { kind: "unavailable" };

/** What to show for `parentId`, from what is already known or the parent query. */
export function parentState(
  parentId: string | null | undefined,
  query: { data?: ParentRef | null | undefined; isPending: boolean; isError: boolean },
  known?: ParentRef,
): ParentState {
  if (!parentId) return { kind: "none" };
  if (known) return { kind: "found", ref: known };
  if (query.data) return { kind: "found", ref: query.data };
  if (query.data === null || query.isError) return { kind: "unavailable" };
  return { kind: "loading" };
}
