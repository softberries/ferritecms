// What the queue can do to a comment, per status (07 §1 Comments; M2b-2 Task 12), and the
// fan-out that does it to many (`fanOut` of `content/bulk.ts`: at most 4 requests in
// flight, failures collected with their problem). A status change is a `PATCH`; a
// permanent delete is only for spam and trash (the server's rule, which also removes the
// replies) and asks first; Reply posts as the signed-in moderator (approved at once).

import { deleteComment, moderateComment } from "@/api/comments";
import { type BulkResult, fanOut } from "@/content/bulk";
import type { CommentStatus } from "./comment-search";

/** The statuses a comment of `status` can be moved to from the queue. */
export function nextStatuses(status: CommentStatus): CommentStatus[] {
  switch (status) {
    case "pending":
      return ["approved", "spam", "trash"];
    case "approved":
      return ["pending", "spam", "trash"];
    case "spam":
    case "trash":
      return ["pending"];
  }
}

/** Whether `DELETE /comments/{id}` takes a comment of `status`. */
export function canDelete(status: CommentStatus): boolean {
  return status === "spam" || status === "trash";
}

export type CommentAction =
  "approve" | "unapprove" | "notSpam" | "restore" | "spam" | "trash" | "reply" | "delete";

/** The actions a row of `status` offers, in the order they show. */
export function actionsFor(status: CommentStatus): CommentAction[] {
  switch (status) {
    case "pending":
      return ["approve", "reply", "spam", "trash"];
    case "approved":
      return ["unapprove", "reply", "spam", "trash"];
    case "spam":
      return ["notSpam", "delete"];
    case "trash":
      return ["restore", "delete"];
  }
}

/** The status an action sets; `null` for Reply and Delete. */
export function statusOf(action: CommentAction): CommentStatus | null {
  switch (action) {
    case "approve":
      return "approved";
    case "unapprove":
    case "notSpam":
    case "restore":
      return "pending";
    case "spam":
      return "spam";
    case "trash":
      return "trash";
    case "reply":
    case "delete":
      return null;
  }
}

/** Sets `status` on every comment of `ids` (4 at a time); failures are listed. */
export function runModeration(
  ids: readonly string[],
  status: CommentStatus,
  apply: (id: string, status: CommentStatus) => Promise<unknown> = moderateComment,
): Promise<BulkResult> {
  return fanOut(
    ids.map((id) => ({ id })),
    ({ id }) => apply(id, status),
  );
}

/** Deletes every comment of `ids` (and their replies, server-side), 4 at a time. */
export function runDeletion(
  ids: readonly string[],
  remove: (id: string) => Promise<unknown> = deleteComment,
): Promise<BulkResult> {
  return fanOut(
    ids.map((id) => ({ id })),
    ({ id }) => remove(id),
  );
}
