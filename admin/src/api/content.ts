import { queryOptions } from "@tanstack/react-query";
import { api, unwrap } from "./client";
import { qk } from "./keys";
import { isProblem } from "./problem";
import type {
  SchemaAutosaveDto,
  SchemaAutosaveInput,
  SchemaContentCountsDto,
  SchemaContentDto,
  SchemaContentInput,
  SchemaContentList,
  SchemaContentPatch,
  SchemaContentRefDto,
  SchemaRevisionDto,
  SchemaRevisionList,
  SchemaScheduleRequest,
  SchemaTranslationCreated,
  operations,
} from "./schema";

/** The `If-Match` header of a content write: the version, quoted (07 §1). */
export function ifMatch(version: number): { "If-Match": string } {
  return { "If-Match": `"${version}"` };
}

/** Every content status, in lifecycle order (`ContentDto.status`; 07 §1). */
export const CONTENT_STATUSES = [
  "draft",
  "pending",
  "scheduled",
  "published",
  "private",
  "trash",
] as const;
export type ContentStatus = (typeof CONTENT_STATUSES)[number];

/** Whether a status string from the API (`status: string` in the DTOs) is a known one. */
export function isContentStatus(status: string): status is ContentStatus {
  return (CONTENT_STATUSES as readonly string[]).includes(status);
}

export type ContentListQuery = NonNullable<operations["list_content"]["parameters"]["query"]>;

/** `signal` cancels the request (an abort rejects with the `AbortError` as is). */
export function listContent(
  type: string,
  query: ContentListQuery = {},
  options: { signal?: AbortSignal } = {},
): Promise<SchemaContentList> {
  return unwrap(
    api.GET("/api/v1/content/{type}", {
      params: { path: { type }, query },
      ...(options.signal ? { signal: options.signal } : {}),
    }),
  );
}

/**
 * `listContent` for one type of a fan-out over every type the sidebar shows (the
 * palette, the relation field, the menu editor): a type deleted since the types were
 * read answers `404 not-found`; it lists nothing and `onGone` runs (the caller reads the
 * types again). Any other failure rejects as before.
 */
export async function listContentOfType(
  type: string,
  query: ContentListQuery,
  options: { signal?: AbortSignal; onGone: () => void },
): Promise<SchemaContentList> {
  try {
    return await listContent(type, query, options.signal ? { signal: options.signal } : {});
  } catch (e) {
    if (!isProblem(e, "not-found")) throw e;
    options.onGone();
    return { items: [], next_cursor: null };
  }
}

export function getContent(type: string, id: string): Promise<SchemaContentDto> {
  return unwrap(api.GET("/api/v1/content/{type}/{id}", { params: { path: { type, id } } }));
}

/**
 * `GET /content/refs`: the items of `ids` (1..=200) the caller may read, with type,
 * title, status and permalink; unknown and unreadable ids are absent.
 */
export function getContentRefs(ids: readonly string[]): Promise<SchemaContentRefDto[]> {
  return unwrap(
    api.GET("/api/v1/content/refs", { params: { query: { ids: ids.join(",") } } }),
  ).then((list) => list.items);
}

export function createContent(type: string, body: SchemaContentInput): Promise<SchemaContentDto> {
  return unwrap(api.POST("/api/v1/content/{type}", { params: { path: { type } }, body }));
}

export function patchContent(
  type: string,
  id: string,
  version: number,
  body: SchemaContentPatch,
): Promise<SchemaContentDto> {
  return unwrap(
    api.PATCH("/api/v1/content/{type}/{id}", {
      params: { path: { type, id }, header: ifMatch(version) },
      body,
    }),
  );
}

/**
 * `POST /content/{type}/{id}/translations`: a draft copy of the item in `locale`, in its
 * translation group, and what it could not carry over (`notes`; M6-1).
 */
export function translateContent(
  type: string,
  id: string,
  locale: string,
): Promise<SchemaTranslationCreated> {
  return unwrap(
    api.POST("/api/v1/content/{type}/{id}/translations", {
      params: { path: { type, id } },
      body: { locale },
    }),
  );
}

/** A lifecycle transition; `trash` is `DELETE` (a trashed item is deleted for good elsewhere). */
export type LifecycleAction = "publish" | "unpublish" | "submit" | "restore" | "schedule" | "trash";

/**
 * What a transition may carry (M6-3): an admin's `override_reason` for publish and
 * schedule in a workflow type; the reviewers and a note of a submit.
 */
export interface LifecycleExtra {
  override_reason?: string;
  reviewers?: string[];
  note?: string;
}

export function lifecycle(
  type: string,
  id: string,
  action: "schedule",
  version: number,
  body: SchemaScheduleRequest,
): Promise<SchemaContentDto>;
export function lifecycle(
  type: string,
  id: string,
  action: Exclude<LifecycleAction, "schedule">,
  version: number,
  extra?: LifecycleExtra,
): Promise<SchemaContentDto>;
export function lifecycle(
  type: string,
  id: string,
  action: LifecycleAction,
  version: number,
  body?: SchemaScheduleRequest | LifecycleExtra,
): Promise<SchemaContentDto> {
  const params = { path: { type, id }, header: ifMatch(version) };
  const extra = (body ?? {}) as LifecycleExtra;
  switch (action) {
    case "publish":
      return unwrap(
        api.POST("/api/v1/content/{type}/{id}/publish", {
          params,
          ...(extra.override_reason ? { body: { override_reason: extra.override_reason } } : {}),
        }),
      );
    case "unpublish":
      return unwrap(api.POST("/api/v1/content/{type}/{id}/unpublish", { params }));
    case "submit": {
      const submit = {
        ...(extra.reviewers && extra.reviewers.length > 0 ? { reviewers: extra.reviewers } : {}),
        ...(extra.note ? { note: extra.note } : {}),
      };
      return unwrap(
        api.POST("/api/v1/content/{type}/{id}/submit", {
          params,
          ...(Object.keys(submit).length > 0 ? { body: submit } : {}),
        }),
      );
    }
    case "restore":
      return unwrap(api.POST("/api/v1/content/{type}/{id}/restore", { params }));
    case "trash":
      return unwrap(api.DELETE("/api/v1/content/{type}/{id}", { params }));
    case "schedule":
      if (body === undefined || !("at" in body)) {
        return Promise.reject(new TypeError("lifecycle(schedule) needs { at }"));
      }
      return unwrap(api.POST("/api/v1/content/{type}/{id}/schedule", { params, body }));
  }
}

/** Deletes a trashed item for good (`DELETE ?force=true`; 07 §1). */
export function deleteContent(type: string, id: string, version: number): Promise<void> {
  return unwrap(
    api.DELETE("/api/v1/content/{type}/{id}", {
      params: { path: { type, id }, query: { force: true }, header: ifMatch(version) },
    }),
  ).then(() => undefined);
}

export function getCounts(type: string): Promise<SchemaContentCountsDto> {
  return unwrap(api.GET("/api/v1/content/{type}/counts", { params: { path: { type } } }));
}

/** The type's status counts (what the actor may read); the list tabs and dashboard tiles. */
export function countsQuery(type: string) {
  return queryOptions({
    queryKey: qk.content.counts(type),
    queryFn: () => getCounts(type),
    staleTime: 30_000,
  });
}

/**
 * The caller's autosave of the item, or `null` when there is none (`204`); a `not-found`
 * `ApiError` when the item is not readable.
 */
export async function getAutosave(type: string, id: string): Promise<SchemaAutosaveDto | null> {
  const slot = await unwrap(
    api.GET("/api/v1/content/{type}/{id}/autosave", { params: { path: { type, id } } }),
  );
  return slot ?? null;
}

export function putAutosave(
  type: string,
  id: string,
  body: SchemaAutosaveInput,
): Promise<SchemaAutosaveDto> {
  return unwrap(
    api.PUT("/api/v1/content/{type}/{id}/autosave", { params: { path: { type, id } }, body }),
  );
}

export function deleteAutosave(type: string, id: string): Promise<void> {
  return unwrap(
    api.DELETE("/api/v1/content/{type}/{id}/autosave", { params: { path: { type, id } } }),
  );
}

export function listRevisions(
  type: string,
  id: string,
  query: NonNullable<operations["list_revisions"]["parameters"]["query"]> = {},
): Promise<SchemaRevisionList> {
  return unwrap(
    api.GET("/api/v1/content/{type}/{id}/revisions", { params: { path: { type, id }, query } }),
  );
}

export function getRevision(type: string, id: string, rid: string): Promise<SchemaRevisionDto> {
  return unwrap(
    api.GET("/api/v1/content/{type}/{id}/revisions/{rid}", {
      params: { path: { type, id, rid } },
    }),
  );
}

export function restoreRevision(
  type: string,
  id: string,
  rid: string,
  version: number,
): Promise<SchemaContentDto> {
  return unwrap(
    api.POST("/api/v1/content/{type}/{id}/revisions/{rid}/restore", {
      params: { path: { type, id, rid }, header: ifMatch(version) },
    }),
  );
}

/** At most this many revisions are kept and listed (07 §1). */
export const MAX_REVISIONS = 50;

/** The item's revisions, newest first (all that are kept); the panel count and the drawer. */
export function revisionsQuery(type: string, id: string) {
  return queryOptions({
    queryKey: qk.content.revisions(type, id),
    queryFn: () => listRevisions(type, id, { limit: MAX_REVISIONS }),
  });
}

/** One revision with its content; revisions never change, so it is never refetched. */
export function revisionQuery(type: string, id: string, rid: string) {
  return queryOptions({
    queryKey: qk.content.revision(type, id, rid),
    queryFn: () => getRevision(type, id, rid),
    staleTime: Infinity,
  });
}
