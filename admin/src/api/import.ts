// The WordPress import (07 §1 Import; `import`, admins only): a start goes through
// `uploadWordpressImport` (`upload.ts`, an XHR for progress); these are the reads.

import { infiniteQueryOptions, queryOptions } from "@tanstack/react-query";
import { api, unwrap } from "./client";
import { qk } from "./keys";
import type {
  SchemaImportCountsDto,
  SchemaImportJobDto,
  SchemaImportOptionsDto,
  SchemaImportRunDto,
  SchemaImportRunPageDto,
} from "./schema";

export type ImportJobDto = SchemaImportJobDto;
export type ImportRunDto = SchemaImportRunDto;
export type ImportCountsDto = SchemaImportCountsDto;
export type ImportOptionsDto = SchemaImportOptionsDto;

/** A run's steps, in the order they run (`ImportRunDto.step`). */
export const IMPORT_STEPS = [
  "parse",
  "pass1",
  "pass2",
  "pass3",
  "pass4",
  "fixup",
  "report",
] as const;
export type ImportStep = (typeof IMPORT_STEPS)[number];

/** The options the server takes when a field is missing (`ImportOptionsDto` defaults). */
export const DEFAULT_IMPORT_OPTIONS: ImportOptionsDto = {
  dry_run: false,
  keep_permalinks: true,
  site_options: true,
  fetch_media: false,
  category_base: "category",
  tag_base: "tag",
};

/** The job kind an import runs as (the System screen's job filter). */
export const IMPORT_WORDPRESS_KIND = "import.wordpress";

/** How often a queued or running job is polled. */
export const IMPORT_POLL_MS = 2_000;
/** The runs the screen lists. */
export const IMPORT_RUNS_PAGE = 20;

/** `GET /import/jobs/{job_id}`: the job and, once it started one, its run. */
export function getImportJob(id: string, signal?: AbortSignal): Promise<ImportJobDto> {
  return unwrap(
    api.GET("/api/v1/import/jobs/{job_id}", {
      params: { path: { job_id: id } },
      ...(signal ? { signal } : {}),
    }),
  );
}

/** `GET /import/runs`: one page of runs, newest first (CLI runs too). */
export function listImportRuns(
  query: { limit?: number; cursor?: string } = {},
  signal?: AbortSignal,
): Promise<SchemaImportRunPageDto> {
  return unwrap(
    api.GET("/api/v1/import/runs", { params: { query }, ...(signal ? { signal } : {}) }),
  );
}

/** Whether a job still moves: polled until it is `done` or `dead`. */
export function jobActive(job: ImportJobDto | undefined): boolean {
  return job?.status === "queued" || job?.status === "running";
}

export const importJobQuery = (id: string) =>
  queryOptions({
    queryKey: qk.import.job(id),
    queryFn: ({ signal }) => getImportJob(id, signal),
    refetchInterval: (query) => (jobActive(query.state.data) ? IMPORT_POLL_MS : false),
  });

/** The runs, newest first, `IMPORT_RUNS_PAGE` a page (`useInfiniteQuery` over `next_cursor`). */
export const importRunsQuery = infiniteQueryOptions({
  queryKey: qk.import.runs,
  queryFn: ({ pageParam, signal }) =>
    listImportRuns(
      { limit: IMPORT_RUNS_PAGE, ...(pageParam ? { cursor: pageParam } : {}) },
      signal,
    ),
  initialPageParam: "",
  getNextPageParam: (last) => last.next_cursor ?? undefined,
});

/** The run's report as a page (opened in a new tab) and as JSON (a download). */
export function reportUrls(runId: string): { html: string; json: string } {
  const base = `/api/v1/import/runs/${encodeURIComponent(runId)}`;
  return { html: `${base}/report.html`, json: `${base}/report.json` };
}
