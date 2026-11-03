// What the content health screen shows of the API's answers (design 16 §11.2).

import type { HealthFinding, HealthSummary } from "@/api/quality";

export const CATEGORIES = ["accessibility", "links", "media", "site", "freshness"] as const;
export type Category = (typeof CATEGORIES)[number];
/** A tab: one category, or all of them. */
export type Tab = "all" | Category;

/** Unsuppressed findings per severity of the newest run (zeros before the first). */
export function severityTotals(summary: HealthSummary | undefined): {
  error: number;
  warning: number;
  notice: number;
} {
  const totals = (summary?.last?.totals ?? {}) as { severity?: Record<string, number> };
  const s = totals.severity ?? {};
  return { error: s.error ?? 0, warning: s.warning ?? 0, notice: s.notice ?? 0 };
}

/** The state of the report: never run, running now, or the last run's outcome. */
export type RunState = "never" | "running" | "done";

export function runState(summary: HealthSummary | undefined): RunState {
  if (summary?.running) return "running";
  if (summary?.last) return "done";
  return "never";
}

/** The editor link of a finding: its item's editor, with `?check=<rule>:<key>`. */
export function findingHref(f: HealthFinding): { to: string; search: { check: string } } | null {
  if (!f.item) return null;
  return {
    to: `/content/${f.item.type}/${f.item.id}`,
    search: { check: `${f.rule}:${f.key}` },
  };
}
