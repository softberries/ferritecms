import { queryOptions } from "@tanstack/react-query";
import { api, unwrap } from "./client";
import { type JobFilters, qk } from "./keys";
import type {
  SchemaJobDto,
  SchemaJobList,
  SchemaMailTestDto,
  SchemaMetricsSummaryDto,
  SchemaSystemStatusDto,
} from "./schema";

export type SystemStatusDto = SchemaSystemStatusDto;
export type MetricsSummaryDto = SchemaMetricsSummaryDto;

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
