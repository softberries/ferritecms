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

/** The local calendar date of `t` as its midnight, epoch milliseconds. */
function localDay(t: number): number {
  const d = new Date(t);
  return new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
}

/**
 * The 7-day chart's day ticks: each bucket whose local date differs from the previous
 * bucket's, i.e. the bucket that holds a local midnight. Buckets start on whole UTC hours,
 * so in a zone with a half-hour offset (India, +5:30) no bucket starts at midnight: the
 * tick is the 00:30 bucket, labelled with its (new) day.
 */
export function dayTicks(rows: readonly MetricPoint[]): number[] {
  const ticks: number[] = [];
  for (let i = 1; i < rows.length; i++) {
    const prev = rows[i - 1];
    const row = rows[i];
    if (prev && row && localDay(prev.t) !== localDay(row.t)) ticks.push(row.t);
  }
  return ticks;
}

/** The bucket starting at `t` is the hour in progress (the window's last, partial one). */
export function inProgress(t: number, now: number): boolean {
  return t + HOUR_MS > now;
}

export interface DayRow {
  /** Local midnight of the day, epoch milliseconds. */
  day: number;
  /** The range holds only part of the day (its first or last, or the hour in progress). */
  partial: boolean;
  pageViews: number;
  requests: number;
  errors: number;
}

/**
 * The points summed per local day, oldest first (the 7-day table: 7 or 8 rows, not 168).
 * A bucket counts for the day its start falls on, so in a half-hour zone the 23:30 bucket
 * belongs to the earlier day; a day is whole when its buckets span its length (23 or 25
 * hours across a DST change) and none is still in progress.
 */
export function byLocalDay(rows: readonly MetricPoint[], now: number): DayRow[] {
  const days = new Map<number, DayRow & { hours: number }>();
  for (const r of rows) {
    const day = localDay(r.t);
    const row = days.get(day) ?? {
      day,
      partial: false,
      hours: 0,
      pageViews: 0,
      requests: 0,
      errors: 0,
    };
    row.hours += 1;
    row.pageViews += r.pageViews;
    row.requests += r.requests;
    row.errors += r.errors;
    if (inProgress(r.t, now)) row.partial = true;
    days.set(day, row);
  }
  return [...days.values()]
    .sort((a, b) => a.day - b.day)
    .map(({ hours, ...row }) => {
      const d = new Date(row.day);
      const next = new Date(d.getFullYear(), d.getMonth(), d.getDate() + 1).getTime();
      return { ...row, partial: row.partial || hours * HOUR_MS < next - row.day };
    });
}

/** Hour and, where the zone's offset is not whole hours, minutes ("2 PM", "2:30 PM"). */
function hourOptions(t: number): Intl.DateTimeFormatOptions {
  return new Date(t).getMinutes() === 0
    ? { hour: "numeric" }
    : { hour: "numeric", minute: "2-digit" };
}

/**
 * An axis tick: the hour over 24 hours ("2 PM"; "2:30 PM" in a half-hour zone), the
 * weekday over 7 ("Thu": the ticks are seven different days; the tooltip has the date).
 */
export function tickFormat(range: MetricsRange): (t: number) => string {
  if (range === "7d") {
    const weekday = new Intl.DateTimeFormat(uiLocale(), { weekday: "short" });
    return (t) => weekday.format(new Date(t));
  }
  return (t) => new Intl.DateTimeFormat(uiLocale(), hourOptions(t)).format(new Date(t));
}

/** A bucket's hour in local time: "Thu, Oct 30, 2 – 3 PM" ("2:30 – 3:30 PM" in India). */
export function hourLabel(t: number): string {
  return new Intl.DateTimeFormat(uiLocale(), {
    weekday: "short",
    month: "short",
    day: "numeric",
    ...hourOptions(t),
  }).formatRange(new Date(t), new Date(t + HOUR_MS));
}

/**
 * Every chart's tooltip title: the hovered bucket's hour, with `soFar` appended while
 * that hour is still in progress (its counts are partial).
 */
export function bucketTooltipLabel(soFar: string) {
  return (_label: unknown, payload: ReadonlyArray<{ payload?: unknown }>): string | null => {
    const at = (payload[0]?.payload as { t?: number } | undefined)?.t;
    if (at === undefined) return null;
    return inProgress(at, Date.now()) ? `${hourLabel(at)} · ${soFar}` : hourLabel(at);
  };
}

/**
 * A hit ratio (0..1) as a percentage that never claims 100 % with a miss: whole percent
 * below 99 %, one decimal rounded down from there ("99.6%"), "100%" only when exact.
 */
export function hitRatioText(ratio: number): string {
  if (ratio >= 1) return new Intl.NumberFormat(uiLocale(), { style: "percent" }).format(1);
  if (ratio >= 0.99) {
    return new Intl.NumberFormat(uiLocale(), {
      style: "percent",
      minimumFractionDigits: 1,
      maximumFractionDigits: 1,
    }).format(Math.floor(ratio * 1000) / 1000);
  }
  return new Intl.NumberFormat(uiLocale(), {
    style: "percent",
    maximumFractionDigits: 0,
  }).format(ratio);
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

/**
 * A plain duration for an axis tick ("250 ms", "5 s", "10 s"): ticks are positions, not
 * bounds, so the 10 s position reads "10 s" though a value drawn there means "> 5 s".
 */
export function durationParts(ms: number): { value: number; unit: "ms" | "s" } {
  return ms >= 1000 ? { value: ms / 1000, unit: "s" } : { value: ms, unit: "ms" };
}

/** A p95 bound as text parts: "≤ 250 ms", "≤ 2.5 s", "> 5 s". */
export function p95Parts(ms: number): P95Parts {
  if (ms >= LAST_BOUND_MS) return { kind: "over", value: NEXT_TO_LAST_BOUND_MS / 1000, unit: "s" };
  if (ms > 1000) return { kind: "le", value: ms / 1000, unit: "s" };
  return { kind: "le", value: ms, unit: "ms" };
}
