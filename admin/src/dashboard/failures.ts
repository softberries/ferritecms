// Plugin failures per hour over the last 24 hours, stacked by plugin (site health card).
// The API buckets per hook and hour (`GET /plugins/{name}/metrics`, every node's); this
// sums the hooks of each plugin and keeps at most `MAX_SERIES` series: the plugins with
// the most failures, the rest folded into "other" (chart-1 … chart-5, never cycled).

import type { SchemaFailureBucketDto } from "@/api/schema";

export const FAILURE_HOURS = 24;
export const MAX_SERIES = 5;
const HOUR_MS = 3_600_000;

export interface PluginFailures {
  name: string;
  failures: readonly SchemaFailureBucketDto[];
}

export interface FailureSeries {
  /** A CSS-safe key (`s0` …), so a plugin name never reaches a custom property name. */
  key: string;
  /** The plugin's name, or `null` for the folded "other" series. */
  plugin: string | null;
  total: number;
}

export interface HourRow {
  /** The hour's start (ms since the epoch). */
  hour: number;
  [series: string]: number;
}

export interface HourlyFailures {
  rows: HourRow[];
  series: FailureSeries[];
  total: number;
}

/** The last `FAILURE_HOURS` whole hours ending with the current one, stacked by plugin. */
export function hourlyFailures(plugins: readonly PluginFailures[], now: Date): HourlyFailures {
  const last = Math.floor(now.getTime() / HOUR_MS) * HOUR_MS;
  const first = last - (FAILURE_HOURS - 1) * HOUR_MS;

  // Failures per plugin per hour slot, inside the window only.
  const perPlugin = plugins.map(({ name, failures }) => {
    const slots = new Array<number>(FAILURE_HOURS).fill(0);
    for (const bucket of failures) {
      const start = new Date(bucket.bucket_start).getTime();
      if (Number.isNaN(start)) continue;
      const slot = Math.floor((start - first) / HOUR_MS);
      if (slot < 0 || slot >= FAILURE_HOURS) continue;
      slots[slot] = (slots[slot] ?? 0) + bucket.count;
    }
    return { name, slots, total: slots.reduce((a, b) => a + b, 0) };
  });

  const failing = perPlugin
    .filter((p) => p.total > 0)
    .sort((a, b) => b.total - a.total || a.name.localeCompare(b.name));
  const own = failing.length > MAX_SERIES ? failing.slice(0, MAX_SERIES - 1) : failing;
  const folded = failing.slice(own.length);

  const series: FailureSeries[] = own.map((p, i) => ({
    key: `s${i}`,
    plugin: p.name,
    total: p.total,
  }));
  if (folded.length > 0) {
    series.push({
      key: `s${own.length}`,
      plugin: null,
      total: folded.reduce((n, p) => n + p.total, 0),
    });
  }

  const rows: HourRow[] = Array.from({ length: FAILURE_HOURS }, (_, slot) => {
    const row: HourRow = { hour: first + slot * HOUR_MS };
    own.forEach((p, i) => {
      row[`s${i}`] = p.slots[slot] ?? 0;
    });
    if (folded.length > 0) {
      row[`s${own.length}`] = folded.reduce((n, p) => n + (p.slots[slot] ?? 0), 0);
    }
    return row;
  });

  return { rows, series, total: failing.reduce((n, p) => n + p.total, 0) };
}
