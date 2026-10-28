// A plugin's metrics (`GET /plugins/{name}/metrics`): failures per hook and hour over 24
// hours, every node's buckets ("All nodes"; the hourly fold of `dashboard/failures.ts`,
// a series per hook), and a table of the answering node's call statistics per hook
// since it started (P48, H-P5: behind a load balancer two refreshes may come from two
// nodes): calls, failures, busy skips, budget cuts, the average, a p95 bound from the
// six duration buckets ("≤ 25 ms") and the longest call. Read again every 30 s while
// the page is visible.

import { useQuery } from "@tanstack/react-query";
import type { TFunction } from "i18next";
import { format } from "date-fns";
import { CircleCheckIcon } from "lucide-react";
import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { Bar, BarChart, CartesianGrid, XAxis, YAxis } from "recharts";
import { INLINE_ERRORS } from "@/api/client";
import { type PluginMetricsDto, pluginMetricsQuery } from "@/api/plugins";
import type { SchemaFailureBucketDto, SchemaHookStatsDto } from "@/api/schema";
import { ErrorState } from "@/components/kit/error-state";
import {
  ChartContainer,
  ChartTooltip,
  ChartTooltipContent,
  type ChartConfig,
} from "@/components/ui/chart";
import { Skeleton } from "@/components/ui/skeleton";
import { FAILURE_HOURS, hourlyFailures } from "@/dashboard/failures";
import { Panel, ScopeBadge } from "@/dashboard/panel";
import { uiLocale } from "@/lib/locale";
import { averageMs, p95Bound } from "./latency";
import { metricsInterval, usePageVisible } from "./plugin-model";

// Series colours in fixed order, as the dashboard's failures chart (never cycled; the
// folded "other hooks" series neutral).
const OWN_COLOURS = [
  "var(--chart-1)",
  "var(--chart-2)",
  "var(--chart-4)",
  "var(--chart-5)",
  "var(--chart-3)",
];
const OTHER_COLOUR = "var(--muted-foreground)";
const colour = (hook: string | null, i: number) =>
  hook === null ? OTHER_COLOUR : (OWN_COLOURS[i] ?? OTHER_COLOUR);

/** The failure buckets grouped by hook, for `hourlyFailures` (a series per hook). */
function byHook(failures: readonly SchemaFailureBucketDto[]) {
  const groups = new Map<string, SchemaFailureBucketDto[]>();
  for (const b of failures) {
    const list = groups.get(b.hook) ?? [];
    list.push(b);
    groups.set(b.hook, list);
  }
  return [...groups].map(([name, buckets]) => ({ name, failures: buckets }));
}

export function PluginMetrics({ name }: { name: string }) {
  const { t } = useTranslation("admin");
  const visible = usePageVisible();
  const metrics = useQuery({
    ...pluginMetricsQuery(name),
    meta: INLINE_ERRORS,
    refetchInterval: metricsInterval(visible),
    refetchIntervalInBackground: false,
  });
  return (
    <div data-slot="plugin-metrics" className="flex min-w-0 flex-col gap-5">
      <Panel
        title={t("plugins.metrics.failuresTitle", { hours: FAILURE_HOURS })}
        scope={<ScopeBadge>{t("plugins.metrics.allNodes")}</ScopeBadge>}
        busy={metrics.isPending}
      >
        {metrics.isError && !metrics.data ? (
          <ErrorState error={metrics.error} retry={() => void metrics.refetch()} />
        ) : metrics.data ? (
          <FailureChart metrics={metrics.data} />
        ) : (
          <Skeleton className="h-40 w-full" />
        )}
      </Panel>
      <Panel
        title={t("plugins.metrics.callsTitle")}
        scope={
          metrics.data ? (
            <ScopeBadge>
              <span data-slot="metrics-scope">
                {t("plugins.metrics.thisNode", {
                  time: format(new Date(metrics.data.node.since), "PPp"),
                })}
              </span>
            </ScopeBadge>
          ) : null
        }
        busy={metrics.isPending}
      >
        {metrics.isError && !metrics.data ? (
          <ErrorState error={metrics.error} retry={() => void metrics.refetch()} />
        ) : metrics.data ? (
          <HookTable hooks={metrics.data.node.hooks} />
        ) : (
          <Skeleton className="h-24 w-full" />
        )}
      </Panel>
    </div>
  );
}

function FailureChart({ metrics }: { metrics: PluginMetricsDto }) {
  const { t } = useTranslation("admin");
  const data = hourlyFailures(byHook(metrics.failures), new Date());
  const number = new Intl.NumberFormat(uiLocale());
  if (data.total === 0) {
    return (
      <Quiet>
        <CircleCheckIcon aria-hidden="true" className="size-4 shrink-0 text-success" />
        {t("plugins.metrics.noFailures", { hours: FAILURE_HOURS })}
      </Quiet>
    );
  }
  const config: ChartConfig = Object.fromEntries(
    data.series.map((s, i) => [
      s.key,
      { label: s.plugin ?? t("plugins.metrics.otherHooks"), color: colour(s.plugin, i) },
    ]),
  );
  const hour = new Intl.DateTimeFormat(uiLocale(), { hour: "numeric" });
  const hourLong = new Intl.DateTimeFormat(uiLocale(), {
    weekday: "short",
    hour: "numeric",
    minute: "2-digit",
  });
  return (
    <>
      <ul className="flex flex-wrap gap-x-4 gap-y-1 text-sm">
        {data.series.map((s, i) => (
          <li key={s.key} className="flex min-w-0 items-center gap-2">
            <span
              aria-hidden="true"
              className="size-2.5 shrink-0 rounded-[3px]"
              style={{ backgroundColor: colour(s.plugin, i) }}
            />
            <span className="min-w-0 font-mono text-xs break-all text-muted-foreground">
              {s.plugin ?? t("plugins.metrics.otherHooks")}
            </span>
            <span className="font-semibold text-foreground tabular-nums">
              {number.format(s.total)}
            </span>
          </li>
        ))}
      </ul>
      <ChartContainer
        config={config}
        className="mt-3 aspect-auto h-40 w-full"
        data-testid="plugin-failures-chart"
        aria-hidden="true"
      >
        <BarChart
          data={data.rows}
          margin={{ top: 4, right: 0, bottom: 0, left: 0 }}
          accessibilityLayer={false}
        >
          <CartesianGrid vertical={false} />
          <XAxis
            dataKey="hour"
            tickLine={false}
            axisLine={false}
            tickMargin={6}
            minTickGap={24}
            tickFormatter={(h: number) => hour.format(new Date(h))}
          />
          <YAxis allowDecimals={false} tickLine={false} axisLine={false} width={28} />
          <ChartTooltip
            cursor
            content={
              <ChartTooltipContent
                labelFormatter={(_, payload) => {
                  const h = (payload[0]?.payload as { hour?: number } | undefined)?.hour;
                  return h === undefined ? null : hourLong.format(new Date(h));
                }}
              />
            }
          />
          {data.series.map((s, i) => (
            <Bar
              key={s.key}
              dataKey={s.key}
              stackId="failures"
              fill={`var(--color-${s.key})`}
              stroke="var(--card)"
              strokeWidth={1}
              maxBarSize={16}
              radius={i === data.series.length - 1 ? [3, 3, 0, 0] : 0}
              isAnimationActive={false}
            />
          ))}
        </BarChart>
      </ChartContainer>
      <p className="sr-only">
        {t("plugins.metrics.failuresSummary", { count: data.total, hours: FAILURE_HOURS })}
      </p>
    </>
  );
}

/** "≤ 25 ms", "> 500 ms", or "–" without calls. */
function p95Text(t: TFunction<"admin">, buckets: readonly number[]): string {
  const bound = p95Bound(buckets);
  if (!bound) return "–";
  return bound.le_ms === null
    ? t("plugins.metrics.p95Over", { ms: 500 })
    : t("plugins.metrics.p95Le", { ms: bound.le_ms });
}

function HookTable({ hooks }: { hooks: readonly SchemaHookStatsDto[] }) {
  const { t } = useTranslation("admin");
  const number = new Intl.NumberFormat(uiLocale());
  const ms = new Intl.NumberFormat(uiLocale(), { maximumFractionDigits: 1 });
  const msText = (v: number | null) =>
    v === null ? "–" : t("plugins.metrics.ms", { ms: ms.format(v) });
  if (hooks.length === 0) {
    return <Quiet>{t("plugins.metrics.noCalls")}</Quiet>;
  }
  const head = "px-3 py-2 text-right font-medium whitespace-nowrap";
  const cell = "px-3 py-2 text-right whitespace-nowrap tabular-nums";
  return (
    // Focusable, so the keyboard can scroll it sideways on a narrow screen.
    <div
      tabIndex={0}
      role="group"
      aria-label={t("plugins.metrics.callsTitle")}
      className="-mx-5 overflow-x-auto focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
    >
      <table data-slot="hook-table" className="w-full text-sm">
        <thead className="border-y bg-muted/40 text-xs text-muted-foreground">
          <tr>
            <th scope="col" className="px-3 py-2 pl-5 text-left font-medium">
              {t("plugins.metrics.hook")}
            </th>
            <th scope="col" className={head}>
              {t("plugins.metrics.calls")}
            </th>
            <th scope="col" className={head}>
              {t("plugins.metrics.failures")}
            </th>
            <th scope="col" className={head}>
              {t("plugins.metrics.busy")}
            </th>
            <th scope="col" className={head}>
              {t("plugins.metrics.budgetCut")}
            </th>
            <th scope="col" className={head}>
              {t("plugins.metrics.average")}
            </th>
            <th scope="col" className={head}>
              {t("plugins.metrics.p95")}
            </th>
            <th scope="col" className={`${head} pr-5`}>
              {t("plugins.metrics.max")}
            </th>
          </tr>
        </thead>
        <tbody>
          {hooks.map((h) => (
            <tr
              key={h.hook}
              data-slot="hook-row"
              data-hook={h.hook}
              className="border-b last:border-b-0"
            >
              <th
                scope="row"
                className="min-w-32 px-3 py-2 pl-5 text-left font-mono text-xs font-normal break-words text-foreground"
              >
                {h.hook}
              </th>
              <td data-col="calls" className={`${cell} text-foreground`}>
                {number.format(h.calls)}
              </td>
              <td
                data-col="failures"
                className={`${cell} ${h.failures > 0 ? "font-medium text-destructive" : "text-muted-foreground"}`}
              >
                {number.format(h.failures)}
              </td>
              <td data-col="busy" className={`${cell} text-muted-foreground`}>
                {number.format(h.busy)}
              </td>
              <td data-col="budget-cut" className={`${cell} text-muted-foreground`}>
                {number.format(h.budget_cut)}
              </td>
              <td data-col="average" className={`${cell} text-foreground`}>
                {msText(averageMs(h.total_micros, h.calls))}
              </td>
              <td data-col="p95" className={`${cell} text-foreground`}>
                {p95Text(t, h.buckets)}
              </td>
              <td data-col="max" className={`${cell} pr-5 text-foreground`}>
                {msText(h.calls === 0 ? null : h.max_micros / 1000)}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function Quiet({ children }: { children: ReactNode }) {
  return (
    <p className="flex items-center gap-2 rounded-lg border border-dashed px-3 py-3 text-sm text-muted-foreground">
      {children}
    </p>
  );
}
