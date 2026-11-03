// `/releases` (M6-3, design 15 §4, §10): named sets of entries (a staged change and/or a
// transition per item) applied in one transaction, now or at a scheduled time. Every write
// of a release carries its `version` as `If-Match` and answers the whole release again.

import { queryOptions } from "@tanstack/react-query";
import { api, unwrap } from "./client";
import { ifMatch } from "./content";
import type {
  SchemaEntryCheckDto,
  SchemaEntryInput,
  SchemaReleaseCheckDto,
  SchemaReleaseDetailDto,
  SchemaReleaseDto,
  SchemaReleaseEntryDto,
  SchemaReleasePatch,
  SchemaReviewDto,
  SchemaStagedChangeDto,
  SchemaSubmitInput,
  SchemaTransitionDto,
} from "./schema";

export type ReleaseDto = SchemaReleaseDto;
export type ReleaseDetail = SchemaReleaseDetailDto;
export type ReleaseEntry = SchemaReleaseEntryDto;
export type ReleaseCheck = SchemaReleaseCheckDto;
export type EntryCheck = SchemaEntryCheckDto;
export type StagedChange = SchemaStagedChangeDto;
export type EntryTransition = SchemaTransitionDto;
export type EntryInput = SchemaEntryInput;

export const RELEASE_STATUSES = ["open", "scheduled", "applied", "failed"] as const;
export type ReleaseStatus = (typeof RELEASE_STATUSES)[number];

export const isReleaseStatus = (s: string): s is ReleaseStatus =>
  (RELEASE_STATUSES as readonly string[]).includes(s);

/** Entries may change (`open` and `failed`; a scheduled one must be unscheduled first). */
export const releaseEditable = (r: Pick<ReleaseDto, "status">) =>
  r.status === "open" || r.status === "failed";

export const MAX_RELEASE_TITLE = 200;
export const MAX_RELEASE_DESCRIPTION = 2000;
export const MAX_OVERRIDE_REASON = 500;

export const releaseKeys = {
  all: ["releases"] as const,
  lists: ["releases", "list"] as const,
  list: (status: string) => ["releases", "list", status] as const,
  item: (id: string) => ["releases", "item", id] as const,
  check: (id: string) => ["releases", "item", id, "check"] as const,
};

export async function listReleases(query: {
  status?: ReleaseStatus;
  mine?: boolean;
}): Promise<ReleaseDto[]> {
  const list = await unwrap(
    api.GET("/api/v1/releases", { params: { query: { limit: 100, ...query } } }),
  );
  return list.items;
}

export function releasesQuery(status: ReleaseStatus | "all") {
  return queryOptions({
    queryKey: releaseKeys.list(status),
    queryFn: () => listReleases(status === "all" ? {} : { status }),
    staleTime: 5_000,
  });
}

export function getRelease(id: string): Promise<ReleaseDetail> {
  return unwrap(api.GET("/api/v1/releases/{id}", { params: { path: { id } } }));
}

export function releaseQuery(id: string) {
  return queryOptions({
    queryKey: releaseKeys.item(id),
    queryFn: () => getRelease(id),
    staleTime: 2_000,
  });
}

export function createRelease(body: {
  title: string;
  description?: string;
}): Promise<ReleaseDetail> {
  return unwrap(api.POST("/api/v1/releases", { body }));
}

export function patchRelease(
  id: string,
  version: number,
  body: SchemaReleasePatch,
): Promise<ReleaseDetail> {
  return unwrap(
    api.PATCH("/api/v1/releases/{id}", {
      params: { path: { id }, header: ifMatch(version) },
      body,
    }),
  );
}

export async function deleteRelease(id: string, version: number): Promise<void> {
  await unwrap(
    api.DELETE("/api/v1/releases/{id}", { params: { path: { id }, header: ifMatch(version) } }),
  );
}

export function addEntry(id: string, version: number, body: EntryInput): Promise<ReleaseDetail> {
  return unwrap(
    api.POST("/api/v1/releases/{id}/items", {
      params: { path: { id }, header: ifMatch(version) },
      body,
    }),
  );
}

/** Replaces the entry of `item` (its staged change and transition). */
export function putEntry(
  id: string,
  item: string,
  version: number,
  body: EntryInput,
): Promise<ReleaseDetail> {
  return unwrap(
    api.PUT("/api/v1/releases/{id}/items/{item}", {
      params: { path: { id, item }, header: ifMatch(version) },
      body,
    }),
  );
}

export function removeEntry(id: string, item: string, version: number): Promise<ReleaseDetail> {
  return unwrap(
    api.DELETE("/api/v1/releases/{id}/items/{item}", {
      params: { path: { id, item }, header: ifMatch(version) },
    }),
  );
}

export function rebaseEntry(id: string, item: string, version: number): Promise<ReleaseDetail> {
  return unwrap(
    api.POST("/api/v1/releases/{id}/items/{item}/rebase", {
      params: { path: { id, item }, header: ifMatch(version) },
    }),
  );
}

export function submitEntry(
  id: string,
  item: string,
  body: SchemaSubmitInput,
): Promise<SchemaReviewDto> {
  return unwrap(
    api.POST("/api/v1/releases/{id}/items/{item}/submit", { params: { path: { id, item } }, body }),
  );
}

export function checkRelease(id: string): Promise<ReleaseCheck> {
  return unwrap(api.GET("/api/v1/releases/{id}/check", { params: { path: { id } } }));
}

export function scheduleRelease(
  id: string,
  version: number,
  at: string,
  overrideReason?: string,
): Promise<ReleaseDetail> {
  return unwrap(
    api.POST("/api/v1/releases/{id}/schedule", {
      params: { path: { id }, header: ifMatch(version) },
      body: { at, ...(overrideReason ? { override_reason: overrideReason } : {}) },
    }),
  );
}

export function unscheduleRelease(id: string, version: number): Promise<ReleaseDetail> {
  return unwrap(
    api.POST("/api/v1/releases/{id}/unschedule", {
      params: { path: { id }, header: ifMatch(version) },
    }),
  );
}

export function applyRelease(
  id: string,
  version: number,
  overrideReason?: string,
): Promise<ReleaseDetail> {
  return unwrap(
    api.POST("/api/v1/releases/{id}/apply", {
      params: { path: { id }, header: ifMatch(version) },
      body: overrideReason ? { override_reason: overrideReason } : {},
    }),
  );
}

/** The release failure stored while `failed` (`{at, reason, entries}`), read defensively. */
export interface ReleaseFailure {
  at: string | null;
  reason: string | null;
  entries: EntryCheck[];
}

export function failureOf(r: Pick<ReleaseDto, "failure">): ReleaseFailure | null {
  const f = r.failure as { at?: unknown; reason?: unknown; entries?: unknown } | null | undefined;
  if (!f || typeof f !== "object") return null;
  return {
    at: typeof f.at === "string" ? f.at : null,
    reason: typeof f.reason === "string" ? f.reason : null,
    entries: Array.isArray(f.entries) ? (f.entries as EntryCheck[]) : [],
  };
}

/** The per-entry report a `release-conflict` / `release-not-ready` problem carries. */
export function problemEntries(problem: { entries?: unknown }): EntryCheck[] {
  return Array.isArray(problem.entries) ? (problem.entries as EntryCheck[]) : [];
}

/** What an entry does, as the chips show it. */
export function entryActions(e: Pick<ReleaseEntry, "has_changes" | "transition">) {
  return {
    changes: e.has_changes,
    publish: e.transition === "publish",
    unpublish: e.transition === "unpublish",
  };
}

/** A staged change from the editor's draft members (a full snapshot, design 15 §4.2). */
export interface StagedSource {
  title: string;
  body: unknown;
  excerpt: string;
  slug: string;
  parent_id: string | null;
  menu_order: number;
  terms: string[];
  fields: unknown;
  seo: unknown;
}

export function stagedChangeOf(s: StagedSource): StagedChange {
  const slug = s.slug.trim();
  return {
    title: s.title,
    body: s.body,
    excerpt: s.excerpt === "" ? null : s.excerpt,
    slug: slug === "" ? null : slug,
    parent_id: s.parent_id,
    menu_order: s.menu_order,
    terms: [...s.terms],
    fields: s.fields,
    seo: s.seo,
  };
}
