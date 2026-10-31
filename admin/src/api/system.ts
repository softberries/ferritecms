import { keepPreviousData, queryOptions } from "@tanstack/react-query";
import { INLINE_ERRORS, api, unwrap } from "./client";
import { type JobFilters, qk } from "./keys";
import type {
  SchemaJobDto,
  SchemaJobList,
  SchemaMailTestDto,
  SchemaMetricsSeriesDto,
  SchemaMetricsSummaryDto,
  SchemaSeriesStepDto,
  SchemaSystemStatusDto,
} from "./schema";

export type SystemStatusDto = SchemaSystemStatusDto;
export type MetricsSummaryDto = SchemaMetricsSummaryDto;
export type MetricsSeriesDto = SchemaMetricsSeriesDto;

/** The dashboard's metrics ranges: hourly buckets over 24 hours or 7 days. */
export const METRICS_RANGES = ["24h", "7d"] as const;
export type MetricsRange = (typeof METRICS_RANGES)[number];
export const RANGE_HOURS: Record<MetricsRange, number> = { "24h": 24, "7d": 168 };

/** How often the dashboard reads a series again (rows are written once a minute). */
export const SERIES_REFRESH_MS = 60_000;

const HOUR_MS = 3_600_000;

export interface SeriesQuery {
  from: string;
  to: string;
  step: SchemaSeriesStepDto;
  node?: string;
}

/**
 * The window of `range` at `now`: whole hours ending with the end of the current one, so
 * the last bucket is the hour in progress. Hourly for both ranges: within the API's
 * limits (an hour series spans at most 7 days) and 24 or 168 points, where a minute
 * series would send 1,440 buckets for a day and a day series has UTC days, not local ones.
 */
export function seriesWindow(range: MetricsRange, now: Date): SeriesQuery {
  const to = Math.floor(now.getTime() / HOUR_MS) * HOUR_MS + HOUR_MS;
  const from = to - RANGE_HOURS[range] * HOUR_MS;
  return { from: new Date(from).toISOString(), to: new Date(to).toISOString(), step: "hour" };
}

/**
 * `GET /system/metrics/series` (`manage_system`): dense buckets, every node's summed
 * (the page latency histograms too, `p95_ms` computed from the sum) unless `node` is given.
 */
export function getMetricsSeries(
  query: SeriesQuery,
  options: { signal?: AbortSignal } = {},
): Promise<MetricsSeriesDto> {
  return unwrap(
    api.GET("/api/v1/system/metrics/series", {
      params: { query },
      ...(options.signal ? { signal: options.signal } : {}),
    }),
  );
}

/**
 * A dashboard range, every node's. The window is taken when the query runs, so the key
 * names only the range and each refetch moves it on. Read again every minute while the
 * page is visible; a range switch keeps the previous figures until the new ones arrive.
 * The cards show their own error (no toast each minute while the server is down).
 */
export function metricsSeriesQuery(range: MetricsRange) {
  return queryOptions({
    queryKey: qk.system.series(range),
    queryFn: ({ signal }) => getMetricsSeries(seriesWindow(range, new Date()), { signal }),
    staleTime: SERIES_REFRESH_MS,
    refetchInterval: SERIES_REFRESH_MS,
    refetchIntervalInBackground: false,
    placeholderData: keepPreviousData,
    meta: INLINE_ERRORS,
  });
}

/** `GET /system/status` (`manage_system`): versions, schema verdict, theme, job counts. */
export function getSystemStatus(): Promise<SystemStatusDto> {
  return unwrap(api.GET("/api/v1/system/status"));
}

/** `GET /system/metrics/summary` (`manage_system`): the answering node's counters. */
export function getMetricsSummary(): Promise<MetricsSummaryDto> {
  return unwrap(api.GET("/api/v1/system/metrics/summary"));
}

/** `GET /system/jobs` (`manage_system`): one page, newest first. */
export function listJobs(
  query: JobFilters = {},
  options: { signal?: AbortSignal } = {},
): Promise<SchemaJobList> {
  return unwrap(
    api.GET("/api/v1/system/jobs", {
      params: { query },
      ...(options.signal ? { signal: options.signal } : {}),
    }),
  );
}

/**
 * `POST /system/jobs/{id}/retry` (`manage_system`): a dead job back to the queue
 * (`handled: false` when no live node runs its kind).
 */
export function retryJob(id: string): Promise<SchemaJobDto> {
  return unwrap(api.POST("/api/v1/system/jobs/{id}/retry", { params: { path: { id } } }));
}

/** `POST /system/mail/test` (`manage_system`): queues a test mail to the caller. */
export function sendTestMail(): Promise<SchemaMailTestDto> {
  return unwrap(api.POST("/api/v1/system/mail/test"));
}

export const systemStatusQuery = queryOptions({
  queryKey: qk.system.status,
  queryFn: getSystemStatus,
  staleTime: 30_000,
});

export const metricsSummaryQuery = queryOptions({
  queryKey: qk.system.metrics,
  queryFn: getMetricsSummary,
  staleTime: 30_000,
});

export function jobsQuery(filters: JobFilters) {
  return queryOptions({
    queryKey: qk.system.jobs(filters),
    queryFn: () => listJobs(filters),
    staleTime: 30_000,
  });
}
