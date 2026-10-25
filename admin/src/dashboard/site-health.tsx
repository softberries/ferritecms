// Site health (`manage_system`): the page-cache hit ratio of the answering node, jobs by
// status, and plugin failures per hour over 24 hours. Cache numbers are node-local
// (`GET /system/metrics/summary`, M2a-2b P20), so that block says "This node only";
// failure buckets are every node's (`GET /plugins/{name}/metrics`, pre-flight F34), so
// the failures chart says "All nodes". Each block loads and fails on its own; numbers
// are always text beside the marks.

import { useQueries, useQuery } from "@tanstack/react-query";
import { CircleCheckIcon } from "lucide-react";
import { type ReactNode, useMemo } from "react";
import { useTranslation } from "react-i18next";
import {
  Bar,
  BarChart,
  CartesianGrid,
  Cell,
  LabelList,
  PolarAngleAxis,
  RadialBar,
  RadialBarChart,
  XAxis,
  YAxis,
} from "recharts";
import { pluginMetricsQuery, pluginsQuery } from "@/api/plugins";
import { metricsSummaryQuery } from "@/api/system";
import { ErrorState } from "@/components/kit/error-state";
import {
  ChartContainer,
  ChartTooltip,
  ChartTooltipContent,
  type ChartConfig,
} from "@/components/ui/chart";
import { Skeleton } from "@/components/ui/skeleton";
import { uiLocale } from "@/lib/locale";
import { FAILURE_HOURS, hourlyFailures } from "./failures";
import { Panel, ScopeBadge } from "./panel";

/** Failures are fetched for at most this many enabled plugins. */
export const MAX_PLUGIN_METRICS = 10;

const JOB_STATUSES = ["queued", "running", "done", "dead"] as const;
const JOB_COLOUR: Record<(typeof JOB_STATUSES)[number], string> = {
  queued: "var(--muted-foreground)",
  running: "var(--info)",
  done: "var(--success)",
  dead: "var(--destructive)",
};

export function SiteHealth({ plugins, className }: { plugins: boolean; className?: string }) {
  const { t } = useTranslation("dashboard");
  const summary = useQuery(metricsSummaryQuery);
  return (
    <Panel
      title={t("health.title")}
      description={t("health.description")}
      busy={summary.isPending}
      className={className}
    >
      <div className="grid gap-6 sm:grid-cols-2">
        <Block title={t("health.cache.title")} scope={t("health.thisNode")}>
          {summary.isError ? (
            <BlockError error={summary.error} retry={() => void summary.refetch()} />
          ) : summary.data ? (
            <CacheRatio {...summary.data.page_cache} />
          ) : (
            <Skeleton className="h-36 w-full" />
          )}
        </Block>
        <Block title={t("health.jobs.title")}>
          {summary.isError ? (
            <BlockError error={summary.error} retry={() => void summary.refetch()} />
          ) : summary.data ? (
            <JobsBar counts={summary.data.jobs} />
          ) : (
            <Skeleton className="h-36 w-full" />
          )}
        </Block>
      </div>
      {plugins ? (
        <div className="mt-6 border-t pt-5">
          <PluginFailures />
        </div>
      ) : null}
    </Panel>
  );
}

function Block({ title, scope, children }: { title: string; scope?: string; children: ReactNode }) {
  return (
    <div className="flex min-w-0 flex-col gap-3">
      <div className="flex flex-wrap items-center gap-2">
        <h3 className="text-sm font-medium text-foreground">{title}</h3>
        {scope ? <ScopeBadge>{scope}</ScopeBadge> : null}
      </div>
      {children}
    </div>
  );
}

function BlockError({ error, retry }: { error: unknown; retry: () => void }) {
  return <ErrorState error={error} retry={retry} className="border-dashed px-4 py-6" />;
}

function formatBytes(bytes: number): string {
  const units = ["byte", "kilobyte", "megabyte", "gigabyte"] as const;
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return new Intl.NumberFormat(uiLocale(), {
    style: "unit",
    unit: units[unit],
    // "0 bytes", "12.4 kB": the long form only for bytes.
    unitDisplay: unit === 0 ? "long" : "short",
    maximumFractionDigits: unit === 0 ? 0 : 1,
  }).format(value);
}

/** The hit ratio as a ring (`hits / (hits + misses)`) with every number as text. */
function CacheRatio({
  hits,
  misses,
  entries,
  bytes,
}: {
  hits: number;
  misses: number;
  entries: number;
  bytes: number;
}) {
  const { t } = useTranslation("dashboard");
  const number = new Intl.NumberFormat(uiLocale());
  const lookups = hits + misses;
  const ratio = lookups === 0 ? null : hits / lookups;
  const percent = new Intl.NumberFormat(uiLocale(), { style: "percent", maximumFractionDigits: 0 });
  const config = { ratio: { label: t("health.cache.ratio"), color: "var(--chart-1)" } };
  return (
    <div className="flex items-center gap-5" data-testid="cache-ratio">
      <div className="relative size-32 shrink-0" aria-hidden="true">
        <ChartContainer config={config} className="aspect-square size-32">
          <RadialBarChart
            data={[{ key: "ratio", value: (ratio ?? 0) * 100, fill: "var(--color-ratio)" }]}
            startAngle={90}
            endAngle={-270}
            innerRadius="78%"
            outerRadius="100%"
            barSize={10}
            accessibilityLayer={false}
          >
            <PolarAngleAxis type="number" domain={[0, 100]} tick={false} axisLine={false} />
            <RadialBar dataKey="value" background cornerRadius={5} isAnimationActive={false} />
          </RadialBarChart>
        </ChartContainer>
        <div className="absolute inset-0 flex flex-col items-center justify-center">
          <span className="text-xl font-semibold text-foreground">
            {ratio === null ? "–" : percent.format(ratio)}
          </span>
          <span className="text-xs text-muted-foreground">{t("health.cache.ratioShort")}</span>
        </div>
      </div>
      <dl className="grid min-w-0 flex-1 grid-cols-[auto_1fr] gap-x-4 gap-y-1.5 text-sm">
        <dt className="sr-only">{t("health.cache.ratio")}</dt>
        <dd className="sr-only">
          {ratio === null ? t("health.cache.noLookups") : percent.format(ratio)}
        </dd>
        <dt className="whitespace-nowrap text-muted-foreground">{t("health.cache.hits")}</dt>
        <dd className="text-right font-medium whitespace-nowrap text-foreground tabular-nums">
          {number.format(hits)}
        </dd>
        <dt className="whitespace-nowrap text-muted-foreground">{t("health.cache.misses")}</dt>
        <dd className="text-right font-medium whitespace-nowrap text-foreground tabular-nums">
          {number.format(misses)}
        </dd>
        <dt className="whitespace-nowrap text-muted-foreground">{t("health.cache.entries")}</dt>
        <dd className="text-right font-medium whitespace-nowrap text-foreground tabular-nums">
          {number.format(entries)}
        </dd>
        <dt className="whitespace-nowrap text-muted-foreground">{t("health.cache.size")}</dt>
        <dd className="text-right font-medium whitespace-nowrap text-foreground tabular-nums">
          {formatBytes(bytes)}
        </dd>
        {ratio === null ? (
          <dd className="col-span-2 text-xs text-muted-foreground" aria-hidden="true">
            {t("health.cache.noLookups")}
          </dd>
        ) : null}
      </dl>
    </div>
  );
}

/** Jobs by status as horizontal bars: the status names the bar, the count sits at its tip. */
function JobsBar({ counts }: { counts: Record<(typeof JOB_STATUSES)[number], number> }) {
  const { t } = useTranslation("dashboard");
  const number = new Intl.NumberFormat(uiLocale());
  const data = JOB_STATUSES.map((status) => ({
    status,
    label: t(`health.jobs.${status}`),
    count: counts[status],
  }));
  const config = { count: { label: t("health.jobs.count") } } satisfies ChartConfig;
  return (
    <div data-testid="jobs-chart">
      <ChartContainer config={config} className="aspect-auto h-36 w-full" aria-hidden="true">
        <BarChart
          data={data}
          layout="vertical"
          margin={{ top: 0, right: 40, bottom: 0, left: 0 }}
          barCategoryGap={6}
          accessibilityLayer={false}
        >
          <XAxis type="number" hide allowDecimals={false} domain={[0, "dataMax"]} />
          <YAxis
            type="category"
            dataKey="label"
            tickLine={false}
            axisLine={false}
            width={72}
            tickMargin={4}
          />
          <Bar
            dataKey="count"
            radius={[0, 4, 4, 0]}
            maxBarSize={20}
            // A stub for zero, so the count label still sits at its tip.
            minPointSize={3}
            isAnimationActive={false}
          >
            {data.map((d) => (
              <Cell key={d.status} fill={JOB_COLOUR[d.status]} />
            ))}
            <LabelList
              dataKey="count"
              position="right"
              className="fill-foreground tabular-nums"
              formatter={(v: unknown) => (typeof v === "number" ? number.format(v) : "")}
            />
          </Bar>
        </BarChart>
      </ChartContainer>
      <ul className="sr-only">
        {data.map((d) => (
          <li key={d.status}>
            {d.label}: {number.format(d.count)}
          </li>
        ))}
      </ul>
    </div>
  );
}

// Colours of the plugins' own series, in fixed order (never cycled): chart-3 (success)
// and chart-4 (warning) are too close for protan/tritan viewers when they touch, so
// chart-3 comes last; the dataviz palette validator passes this order in both modes
// (adjacent CVD ΔE ≥ 11). The folded "Other plugins" series is neutral, never green
// ("OK") on a failures chart.
const OWN_COLOURS = [
  "var(--chart-1)",
  "var(--chart-2)",
  "var(--chart-4)",
  "var(--chart-5)",
  "var(--chart-3)",
];
const OTHER_COLOUR = "var(--muted-foreground)";

function seriesColour(plugin: string | null, index: number): string {
  return plugin === null ? OTHER_COLOUR : (OWN_COLOURS[index] ?? OTHER_COLOUR);
}

/** Plugin failures per hour over 24 hours, stacked by plugin (every node's buckets). */
function PluginFailures() {
  const { t } = useTranslation("dashboard");
  const plugins = useQuery(pluginsQuery);
  const enabled = useMemo(
    () => (plugins.data ?? []).filter((p) => p.enabled).slice(0, MAX_PLUGIN_METRICS),
    [plugins.data],
  );
  const metrics = useQueries({ queries: enabled.map((p) => pluginMetricsQuery(p.name)) });
  const failed = plugins.isError ? plugins : metrics.find((q) => q.isError);
  const loaded = plugins.data !== undefined && metrics.every((q) => q.data);
  // Cheap (24 rows × at most 10 plugins), so computed on every render.
  const data = loaded
    ? hourlyFailures(
        enabled.map((p, i) => ({ name: p.name, failures: metrics[i]?.data?.failures ?? [] })),
        new Date(),
      )
    : null;
  const config: ChartConfig = Object.fromEntries(
    (data?.series ?? []).map((s, i) => [
      s.key,
      { label: s.plugin ?? t("health.failures.other"), color: seriesColour(s.plugin, i) },
    ]),
  );
  const hour = new Intl.DateTimeFormat(uiLocale(), { hour: "numeric" });
  const hourLong = new Intl.DateTimeFormat(uiLocale(), {
    weekday: "short",
    hour: "numeric",
    minute: "2-digit",
  });
  const number = new Intl.NumberFormat(uiLocale());

  let body;
  if (failed) {
    body = (
      <BlockError
        error={failed.error}
        retry={() => {
          if (plugins.isError) void plugins.refetch();
          for (const q of metrics) if (q.isError) void q.refetch();
        }}
      />
    );
  } else if (!data) {
    body = <Skeleton className="h-40 w-full" />;
  } else if (enabled.length === 0) {
    body = <Quiet>{t("health.failures.noPlugins")}</Quiet>;
  } else if (data.total === 0) {
    body = <Quiet ok>{t("health.failures.none", { hours: FAILURE_HOURS })}</Quiet>;
  } else {
    body = (
      <>
        <ul className="flex flex-wrap gap-x-4 gap-y-1 text-sm">
          {data.series.map((s, i) => (
            <li key={s.key} className="flex items-center gap-2">
              <span
                aria-hidden="true"
                className="size-2.5 rounded-[3px]"
                style={{ backgroundColor: seriesColour(s.plugin, i) }}
              />
              <span className="text-muted-foreground">
                {s.plugin ?? t("health.failures.other")}
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
          data-testid="failures-chart"
        >
          <BarChart data={data.rows} margin={{ top: 4, right: 0, bottom: 0, left: 0 }}>
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
          {t("health.failures.summary", { count: data.total, hours: FAILURE_HOURS })}
        </p>
      </>
    );
  }

  return (
    <Block
      title={t("health.failures.title", { hours: FAILURE_HOURS })}
      scope={t("health.allNodes")}
    >
      {body}
      {plugins.data && plugins.data.filter((p) => p.enabled).length > MAX_PLUGIN_METRICS ? (
        <p className="text-xs text-muted-foreground">
          {t("health.failures.capped", { count: MAX_PLUGIN_METRICS })}
        </p>
      ) : null}
    </Block>
  );
}

function Quiet({ ok = false, children }: { ok?: boolean; children: ReactNode }) {
  return (
    <p className="flex items-center gap-2 rounded-lg border border-dashed px-3 py-3 text-sm text-muted-foreground">
      {ok ? <CircleCheckIcon aria-hidden="true" className="size-4 shrink-0 text-success" /> : null}
      {children}
    </p>
  );
}
