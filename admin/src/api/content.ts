import { api, unwrap } from "./client";
import type {
  SchemaAutosaveDto,
  SchemaAutosaveInput,
  SchemaContentCountsDto,
  SchemaContentDto,
  SchemaContentInput,
  SchemaContentList,
  SchemaContentPatch,
  SchemaRevisionDto,
  SchemaRevisionList,
  SchemaScheduleRequest,
  operations,
} from "./schema";

/** The `If-Match` header of a content write: the version, quoted (07 §1). */
export function ifMatch(version: number): { "If-Match": string } {
  return { "If-Match": `"${version}"` };
}

export type ContentListQuery = NonNullable<operations["list_content"]["parameters"]["query"]>;

export function listContent(
  type: string,
  query: ContentListQuery = {},
): Promise<SchemaContentList> {
  return unwrap(api.GET("/api/v1/content/{type}", { params: { path: { type }, query } }));
}

export function getContent(type: string, id: string): Promise<SchemaContentDto> {
  return unwrap(api.GET("/api/v1/content/{type}/{id}", { params: { path: { type, id } } }));
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

/** A lifecycle transition; `trash` is `DELETE` (a trashed item is deleted for good elsewhere). */
export type LifecycleAction = "publish" | "unpublish" | "submit" | "restore" | "schedule" | "trash";

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
): Promise<SchemaContentDto>;
export function lifecycle(
  type: string,
  id: string,
  action: LifecycleAction,
  version: number,
  body?: SchemaScheduleRequest,
): Promise<SchemaContentDto> {
  const params = { path: { type, id }, header: ifMatch(version) };
  switch (action) {
    case "publish":
      return unwrap(api.POST("/api/v1/content/{type}/{id}/publish", { params }));
    case "unpublish":
      return unwrap(api.POST("/api/v1/content/{type}/{id}/unpublish", { params }));
    case "submit":
      return unwrap(api.POST("/api/v1/content/{type}/{id}/submit", { params }));
    case "restore":
      return unwrap(api.POST("/api/v1/content/{type}/{id}/restore", { params }));
    case "trash":
      return unwrap(api.DELETE("/api/v1/content/{type}/{id}", { params }));
    case "schedule":
      if (body === undefined) {
        return Promise.reject(new TypeError("lifecycle(schedule) needs { at }"));
      }
      return unwrap(api.POST("/api/v1/content/{type}/{id}/schedule", { params, body }));
  }
}

export function getCounts(type: string): Promise<SchemaContentCountsDto> {
  return unwrap(api.GET("/api/v1/content/{type}/counts", { params: { path: { type } } }));
}

/** The caller's autosave of the item; a `not-found` `ApiError` when there is none. */
export function getAutosave(type: string, id: string): Promise<SchemaAutosaveDto> {
  return unwrap(
    api.GET("/api/v1/content/{type}/{id}/autosave", { params: { path: { type, id } } }),
  );
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
