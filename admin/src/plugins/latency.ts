// Hook latency from the host's six duration buckets (`HookStatsDto.buckets`: calls that
// took at most 1, 5, 25, 100 and 500 ms, and longer). The host keeps no finer histogram,
// so the 95th percentile is shown as the bucket bound that holds it ("p95 ≤ 25 ms", P48).

/** The upper bounds of the first five buckets; the sixth is "longer". */
export const BUCKET_BOUNDS_MS = [1, 5, 25, 100, 500] as const;

export interface P95Bound {
  /** The smallest bound whose cumulative count reaches 95 % of calls; `null`: over 500 ms. */
  le_ms: number | null;
}

/** The p95 bound of `buckets`; `null` without calls (no value to show). */
export function p95Bound(buckets: readonly number[]): P95Bound | null {
  const total = buckets.reduce((n, b) => n + b, 0);
  if (total === 0) return null;
  let cumulative = 0;
  for (let i = 0; i < BUCKET_BOUNDS_MS.length; i++) {
    cumulative += buckets[i] ?? 0;
    // `cumulative / total >= 0.95` in integers.
    if (cumulative * 100 >= total * 95) return { le_ms: BUCKET_BOUNDS_MS[i] ?? null };
  }
  return { le_ms: null };
}

/** The mean duration in milliseconds; `null` without calls. */
export function averageMs(totalMicros: number, calls: number): number | null {
  return calls === 0 ? null : totalMicros / calls / 1000;
}
