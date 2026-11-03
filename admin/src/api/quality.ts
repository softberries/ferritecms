// Content quality (M6-4, design 16 §10): the editor's check, suppressions, and the
// content health report (`edit_others`).

import { infiniteQueryOptions, queryOptions } from "@tanstack/react-query";
import { api, unwrap } from "./client";
import { qk } from "./keys";
import type {
  SchemaCheckRequestDto,
  SchemaCheckResponseDto,
  SchemaFindingDto,
  SchemaHealthFindingDto,
  SchemaHealthFindingList,
  SchemaHealthSummaryDto,
  SchemaRuleId,
  SchemaSuppressionDto,
  operations,
} from "./schema";

export type CheckRequest = SchemaCheckRequestDto;
export type CheckResponse = SchemaCheckResponseDto;
export type FindingDto = SchemaFindingDto;
export type RuleId = SchemaRuleId;
export type SuppressionDto = SchemaSuppressionDto;
export type HealthSummary = SchemaHealthSummaryDto;
export type HealthFinding = SchemaHealthFindingDto;
export type HealthFindingList = SchemaHealthFindingList;

/** The filters of the health report (every query member but the page cursor and size). */
export type HealthFilters = Omit<
  NonNullable<operations["health_findings"]["parameters"]["query"]>,
  "cursor" | "limit"
>;

/** Every rule id, in the catalogue's order (`quality::RULES`; a test keeps them equal). */
export const RULE_IDS = [
  "image-alt-missing",
  "image-alt-empty",
  "image-alt-filename",
  "heading-empty",
  "heading-level-skipped",
  "link-text-empty",
  "link-text-vague",
  "link-text-url",
  "table-header-missing",
  "table-header-empty",
  "html-unchecked",
  "media-missing",
  "featured-alt-missing",
  "link-internal-broken",
  "link-internal-redirected",
  "link-anchor-missing",
  "link-external-broken",
  "orphan",
  "redirect-chain",
  "redirect-loop",
  "redirect-target-broken",
  "stale",
  "check-truncated",
  "findings-truncated",
] as const satisfies readonly RuleId[];

/** `POST /content/{type}/check`: the draft's findings, counts and the type's gate. */
export function checkContent(
  type: string,
  draft: CheckRequest,
  signal?: AbortSignal,
): Promise<CheckResponse> {
  return unwrap(
    api.POST("/api/v1/content/{type}/check", {
      params: { path: { type } },
      body: draft,
      ...(signal ? { signal } : {}),
    }),
  );
}

export function suppress(
  type: string,
  id: string,
  body: { rule: RuleId; key: string; reason: string },
): Promise<SuppressionDto> {
  return unwrap(
    api.POST("/api/v1/content/{type}/{id}/suppressions", {
      params: { path: { type, id } },
      body,
    }),
  );
}

export async function unsuppress(type: string, id: string, rule: RuleId, key: string) {
  await unwrap(
    api.DELETE("/api/v1/content/{type}/{id}/suppressions/{rule}", {
      params: { path: { type, id, rule }, query: { key } },
    }),
  );
}

export const healthSummaryQuery = queryOptions({
  queryKey: qk.health.summary,
  queryFn: () => unwrap(api.GET("/api/v1/health/summary")),
  staleTime: 15_000,
});

/** One page of findings. */
export const HEALTH_PAGE = 50;

export function healthFindingsQuery(filters: HealthFilters) {
  return infiniteQueryOptions({
    queryKey: qk.health.findings(filters),
    queryFn: ({ pageParam }) =>
      unwrap(
        api.GET("/api/v1/health/findings", {
          params: {
            query: {
              ...filters,
              limit: HEALTH_PAGE,
              ...(pageParam ? { cursor: pageParam } : {}),
            },
          },
        }),
      ),
    initialPageParam: null as string | null,
    getNextPageParam: (last: HealthFindingList) => last.next_cursor ?? null,
    staleTime: 15_000,
  });
}

/** "Run now": `202` with the job, `409 conflict` while one is queued or running. */
export function requestHealthRun(): Promise<{ job_id: string }> {
  return unwrap(api.POST("/api/v1/health/runs"));
}
