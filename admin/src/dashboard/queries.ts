// What the dashboard cards read and who may see what. Every list here is under the
// type's `lists` key and the counts under `counts`, so a content save refreshes them.

import { type ContentFilters } from "@/api/keys";
import type { ContentSummaryDto } from "./activity";

/** Entries per list card (recent drafts, pending review, dead jobs). */
export const CARD_LIMIT = 5;

/** The actor's latest drafts: `author` is the actor's id (pre-flight F34). */
export function draftsFilters(author: string): ContentFilters {
  return { author, status: "draft", order: "updated", limit: CARD_LIMIT };
}

export const PENDING_FILTERS: ContentFilters = {
  status: "pending",
  order: "updated",
  limit: CARD_LIMIT,
};

/** The newest `limit` items of several lists, by `updated_at`. */
export function newest(
  lists: readonly (readonly ContentSummaryDto[])[],
  limit: number,
): ContentSummaryDto[] {
  return lists
    .flat()
    .sort((a, b) => Date.parse(b.updated_at) - Date.parse(a.updated_at))
    .slice(0, limit);
}

export interface AttentionAccess {
  pending: boolean;
  /** Pending comments (`moderate_comments`). */
  comments: boolean;
  system: boolean;
  plugins: boolean;
}

/**
 * Which checks of Needs attention the actor may see, from `me.capabilities` (the server
 * decides; the SPA only hides): pending review needs `edit_others`, pending comments
 * `moderate_comments` (`GET /comments/counts`), dead jobs, schema
 * and theme `manage_system` (`/system/*`), disabled plugins `manage_plugins`.
 */
export function attentionAccess(capabilities: readonly string[]): AttentionAccess {
  return {
    pending: capabilities.includes("edit_others"),
    comments: capabilities.includes("moderate_comments"),
    system: capabilities.includes("manage_system"),
    plugins: capabilities.includes("manage_plugins"),
  };
}

export function hasAttention(access: AttentionAccess): boolean {
  return access.pending || access.comments || access.system || access.plugins;
}
