import { queryOptions } from "@tanstack/react-query";
import { api, unwrap } from "./client";
import { type CommentFilters, qk } from "./keys";
import type {
  SchemaCommentCountsDto,
  SchemaCommentDto,
  SchemaCommentList,
  SchemaCommentPublicDto,
  SchemaCommentSubmit,
} from "./schema";

export type CommentDto = SchemaCommentDto;

/** A comment's moderation status (07 §1). */
export const COMMENT_STATUSES = ["pending", "approved", "spam", "trash"] as const;
export type CommentStatus = (typeof COMMENT_STATUSES)[number];

/** `GET /comments`: one page, newest first. */
export function listComments(
  query: CommentFilters & { cursor?: string; limit?: number } = {},
  options: { signal?: AbortSignal } = {},
): Promise<SchemaCommentList> {
  return unwrap(
    api.GET("/api/v1/comments", {
      params: { query },
      ...(options.signal ? { signal: options.signal } : {}),
    }),
  );
}

/** `GET /comments/counts`: per status, of one content item when `item` is given. */
export function commentCounts(item?: string): Promise<SchemaCommentCountsDto> {
  return unwrap(
    api.GET("/api/v1/comments/counts", { params: { query: item === undefined ? {} : { item } } }),
  );
}

/** The counts per status (of one item when given); the nav badge, the dashboard, the tabs. */
export function commentCountsQuery(item?: string) {
  return queryOptions({
    queryKey: qk.comments.counts(item),
    queryFn: () => commentCounts(item),
    staleTime: 30_000,
  });
}

/** `status`: `pending`, `approved`, `spam` or `trash`. */
export function moderateComment(id: string, status: CommentStatus): Promise<CommentDto> {
  return unwrap(api.PATCH("/api/v1/comments/{id}", { params: { path: { id } }, body: { status } }));
}

export async function deleteComment(id: string): Promise<void> {
  await unwrap(api.DELETE("/api/v1/comments/{id}", { params: { path: { id } } }));
}

/** A reply as the signed-in user (`POST /content/{type}/{id}/comments`). */
export function replyComment(
  type: string,
  itemId: string,
  body: Pick<SchemaCommentSubmit, "body"> & { parent_id: string },
): Promise<SchemaCommentPublicDto> {
  return unwrap(
    api.POST("/api/v1/content/{type}/{id}/comments", {
      params: { path: { type, id: itemId } },
      body,
    }),
  );
}
