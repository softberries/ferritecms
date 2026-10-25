// What the dashboard cards read and who may see what. Every list here is under the
// type's `lists` key and the counts under `counts`, so a content save refreshes them.

import { queryOptions } from "@tanstack/react-query";
import { getCounts } from "@/api/content";
import { type ContentFilters, qk } from "@/api/keys";
import type { ContentSummaryDto } from "./activity";

/** Entries per list card (recent drafts, pending review, dead jobs). */
export const CARD_LIMIT = 5;

export function countsQuery(type: string) {
  return queryOptions({
    queryKey: qk.content.counts(type),
    queryFn: () => getCounts(type),
    staleTime: 30_000,
  });
}

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
  system: boolean;
  plugins: boolean;
}

/**
 * Which checks of Needs attention the actor may see, from `me.capabilities` (the server
 * decides; the SPA only hides): pending review needs `edit_others`, dead jobs, schema
 * and theme `manage_system` (`/system/*`), disabled plugins `manage_plugins`.
 */
export function attentionAccess(capabilities: readonly string[]): AttentionAccess {
  return {
    pending: capabilities.includes("edit_others"),
    system: capabilities.includes("manage_system"),
    plugins: capabilities.includes("manage_plugins"),
  };
}

export function hasAttention(access: AttentionAccess): boolean {
  return access.pending || access.system || access.plugins;
}
