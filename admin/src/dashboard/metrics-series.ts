// The dashboard's view of `GET /system/metrics/series` (P36): chart points per bucket,
// the range's totals and the 7-day text twin's local days. The buckets are dense and
// every node's, summed; `p95_ms` is the upper bound of the latency bucket holding the
// page p95 (the largest of the bucket's minutes and nodes), 0 without page responses.

import type { SchemaSeriesBucketDto } from "@/api/schema";
import type { MetricsRange } from "@/api/system";
import { uiLocale } from "@/lib/locale";

const HOUR_MS = 3_600_000;

export interface MetricPoint {
  /** The bucket's start, epoch milliseconds. */
  t: number;
  pageViews: number;
  requests: number;
  errors: number;
  hits: number;
  misses: number;
  /** Hits per lookup in percent; `null` without lookups (a gap, not 0 %). */
  hitRatio: number | null;
  /** The page p95 bound in ms; `null` without page responses. */
  p95: number | null;
}

export interface MetricTotals {
  pageViews: number;
  requests: number;
  errors: number;
  hits: number;
  misses: number;
  /** The slowest bucket's p95 bound; `null` when no bucket had page responses. */
  p95: number | null;
}

export function points(buckets: readonly SchemaSeriesBucketDto[]): MetricPoint[] {
  return buckets.map((b) => {
    const lookups = b.cache_hits + b.cache_misses;
    return {
      t: Date.parse(b.start),
      pageViews: b.page_views,
      requests: b.requests,
      errors: b.errors_5xx,
      hits: b.cache_hits,
      misses: b.cache_misses,
      hitRatio: lookups === 0 ? null : (b.cache_hits / lookups) * 100,
      p95: b.p95_ms > 0 ? b.p95_ms : null,
    };
  });
}

export function summarise(buckets: readonly SchemaSeriesBucketDto[]): MetricTotals {
  const totals: MetricTotals = {
    pageViews: 0,
    requests: 0,
    errors: 0,
    hits: 0,
    misses: 0,
    p95: null,
  };
  for (const b of buckets) {
    totals.pageViews += b.page_views;
    totals.requests += b.requests;
    totals.errors += b.errors_5xx;
    totals.hits += b.cache_hits;
    totals.misses += b.cache_misses;
    if (b.p95_ms > 0) totals.p95 = Math.max(totals.p95 ?? 0, b.p95_ms);
  }
  return totals;
}

export function hasTraffic(t: MetricTotals): boolean {
  return t.requests + t.pageViews + t.errors > 0;
}

export function hasPerformance(t: MetricTotals): boolean {
  return t.hits + t.misses > 0 || t.p95 !== null;
}

/** The bucket starts that fall on a local midnight: the 7-day chart's day ticks. */
export function dayTicks(rows: readonly MetricPoint[]): number[] {
  return rows
    .filter((r) => {
      const d = new Date(r.t);
      return d.getHours() === 0 && d.getMinutes() === 0;
    })
    .map((r) => r.t);
}

export interface DayRow {
  /** Local midnight of the day, epoch milliseconds. */
  day: number;
  pageViews: number;
  requests: number;
  errors: number;
}

/** The points summed per local day, oldest first (the 7-day table: 7 or 8 rows, not 168). */
export function byLocalDay(rows: readonly MetricPoint[]): DayRow[] {
  const days = new Map<number, DayRow>();
  for (const r of rows) {
    const d = new Date(r.t);
    const day = new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
    const row = days.get(day) ?? { day, pageViews: 0, requests: 0, errors: 0 };
    row.pageViews += r.pageViews;
    row.requests += r.requests;
    row.errors += r.errors;
    days.set(day, row);
  }
  return [...days.values()].sort((a, b) => a.day - b.day);
}

/** An axis tick: the hour over 24 hours ("2 PM"), the weekday over 7 ("Thu": seven midnights, each a different day; the tooltip has the date). */
export function tickFormat(range: MetricsRange): (t: number) => string {
  const format = new Intl.DateTimeFormat(
    uiLocale(),
    range === "24h" ? { hour: "numeric" } : { weekday: "short" },
  );
  return (t) => format.format(new Date(t));
}

/** A bucket's hour in local time, for tooltips: "Thu, Oct 30, 2 – 3 PM". */
export function hourLabel(t: number): string {
  return new Intl.DateTimeFormat(uiLocale(), {
    weekday: "short",
    month: "short",
    day: "numeric",
    hour: "numeric",
  }).formatRange(new Date(t), new Date(t + HOUR_MS));
}

/** A local day for the 7-day table: "Thursday, October 30". */
export function dayLabel(day: number): string {
  return new Date(day).toLocaleDateString(uiLocale(), {
    weekday: "long",
    month: "long",
    day: "numeric",
  });
}

/** The largest bound, which also holds anything slower (5 s and up). */
const LAST_BOUND_MS = 10_000;
const NEXT_TO_LAST_BOUND_MS = 5_000;

export interface P95Parts {
  /** `le`: at most `value`; `over`: slower than `value`. */
  kind: "le" | "over";
  value: number;
  unit: "ms" | "s";
}

/** A p95 bound as text parts: "≤ 250 ms", "≤ 2.5 s", "> 5 s". */
export function p95Parts(ms: number): P95Parts {
  if (ms >= LAST_BOUND_MS) return { kind: "over", value: NEXT_TO_LAST_BOUND_MS / 1000, unit: "s" };
  if (ms > 1000) return { kind: "le", value: ms / 1000, unit: "s" };
  return { kind: "le", value: ms, unit: "ms" };
}
