// `/change-sets` (M6-5, design 17 §9): what AI assistants changed over MCP, grouped in
// change sets; the detail with its changes, one change's before/after images, and
// revert (a signed-in session only: the admin always is one).

import { queryOptions } from "@tanstack/react-query";
import { api, unwrap } from "./client";
import { qk } from "./keys";
import type {
  SchemaChangeDetailDto,
  SchemaChangeDto,
  SchemaChangeSetDetailDto,
  SchemaChangeSetDto,
  SchemaChangeObjectDto,
  SchemaChangeSetList,
  SchemaRevertInput,
  SchemaRevertReportDto,
} from "./schema";

export type ChangeSetDto = SchemaChangeSetDto;
export type ChangeDto = SchemaChangeDto;
export type ChangeSetDetailDto = SchemaChangeSetDetailDto;
export type ChangeDetailDto = SchemaChangeDetailDto;
export type RevertInput = SchemaRevertInput;
export type RevertReportDto = SchemaRevertReportDto;

/**
 * One entry of a `409 change-conflict`'s `report.conflicts` (`RevertConflictDto` in
 * `crates/ferrite-http/src/api/dto/change_sets.rs`; the problem's `report` is untyped in
 * the spec, so this mirrors it).
 */
export interface RevertConflictDto {
  seq: number;
  reason: "changed_since" | "later_change" | "gone" | "too_large";
  object: SchemaChangeObjectDto;
  by?: string | null;
  at?: string | null;
  later?: number | null;
}
export type ChangeSetStatus = "open" | "closed" | "reverted" | "partly_reverted";

/** The list's filters (`?user=all` for administrators, `?agent=<grant or token>`). */
export interface ChangeSetFilter {
  user?: string;
  agent?: string;
  open?: boolean;
}

export const CHANGE_SET_PAGE = 50;

export function listChangeSets(
  filter: ChangeSetFilter,
  cursor?: string | null,
): Promise<SchemaChangeSetList> {
  return unwrap(
    api.GET("/api/v1/change-sets", {
      params: {
        query: {
          user: filter.user,
          agent: filter.agent,
          open: filter.open,
          limit: CHANGE_SET_PAGE,
          cursor: cursor ?? undefined,
        },
      },
    }),
  );
}

export function changeSetsQuery(filter: ChangeSetFilter) {
  return queryOptions({
    queryKey: qk.changeSets.list(filter),
    queryFn: () => listChangeSets(filter),
    staleTime: 10_000,
  });
}

export function changeSetQuery(id: string) {
  return queryOptions({
    queryKey: qk.changeSets.item(id),
    queryFn: () => unwrap(api.GET("/api/v1/change-sets/{id}", { params: { path: { id } } })),
    staleTime: 10_000,
  });
}

export function changeQuery(id: string, seq: number) {
  return queryOptions({
    queryKey: qk.changeSets.change(id, seq),
    queryFn: () =>
      unwrap(api.GET("/api/v1/change-sets/{id}/changes/{seq}", { params: { path: { id, seq } } })),
    staleTime: 60_000,
  });
}

/** `POST /change-sets/{id}/revert`; a `409 change-conflict` carries `report.conflicts`. */
export function revertChangeSet(id: string, body: RevertInput): Promise<RevertReportDto> {
  return unwrap(api.POST("/api/v1/change-sets/{id}/revert", { params: { path: { id } }, body }));
}
