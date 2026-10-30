// Performance (`manage_system`, P36): the page-cache hit ratio over the range (hits per
// lookup, every node's) and the page p95, each as a headline number with an hourly line
// under it. The p95 is a bucket bound ("≤ 250 ms"), the slowest hour's; hours without
// lookups or page responses are gaps, not zeros. Two separate charts, never two y-scales.

import { useQuery } from "@tanstack/react-query";
import { cn } from "cn";
import { GaugeIcon } from "lucide-react";
import { type ReactNode, useMemo } from "react";
import { useTranslation } from "react-i18next";
import { CartesianGrid, type DotItemDotProps, Line, LineChart, XAxis, YAxis } from "recharts";
import { type MetricsRange, metricsSeriesQuery } from "@/api/system";
import { EmptyState } from "@/components/kit/empty-state";
import {
  ChartContainer,
  ChartTooltip,
  ChartTooltipContent,
  type ChartConfig,
} from "@/components/ui/chart";
import { Skeleton } from "@/components/ui/skeleton";
import { uiLocale } from "@/lib/locale";
import {
  type MetricPoint,
  bucketTooltipLabel,
  dayTicks,
  durationParts,
  hasPerformance,
  hitRatioText,
  p95Parts,
  points,
  summarise,
  tickFormat,
} from "./metrics-series";
import { Panel, PanelError, ScopeBadge, StaleNote } from "./panel";

const MARGIN = { top: 6, right: 4, bottom: 0, left: 0 };
const Y_WIDTH = 56;

/** An axis position as a plain duration ("250 ms", "10 s"), never a bound. */
function useDurationText(): (ms: number) => string {
  const { t } = useTranslation("dashboard");
  const number = new Intl.NumberFormat(uiLocale(), { maximumFractionDigits: 1 });
  return (ms) => {
    const d = durationParts(ms);
    return t(d.unit === "s" ? "performance.s" : "performance.ms", {
      value: number.format(d.value),
    });
  };
}

/** "≤ 250 ms", "≤ 2.5 s", "> 5 s". */
function useP95Text(): (ms: number) => string {
  const { t } = useTranslation("dashboard");
  const number = new Intl.NumberFormat(uiLocale(), { maximumFractionDigits: 1 });
  return (ms) => {
    const p = p95Parts(ms);
    const key =
      p.kind === "over"
        ? "performance.over_s"
        : p.unit === "s"
          ? "performance.le_s"
          : "performance.le_ms";
    return t(key, { value: number.format(p.value) });
  };
}

export function PerformanceCard({ range, className }: { range: MetricsRange; className?: string }) {
  const { t } = useTranslation("dashboard");
  const series = useQuery(metricsSeriesQuery(range));
  const span = t(`metrics.span.${range}`);
  const data = series.data;
  const rows = useMemo(() => (data ? points(data.buckets) : null), [data]);
  const totals = useMemo(() => (data ? summarise(data.buckets) : null), [data]);
  const p95Text = useP95Text();
  const number = new Intl.NumberFormat(uiLocale());
  const durationText = useDurationText();

  let body;
  if (series.isError && !data) {
    body = <PanelError error={series.error} retry={() => void series.refetch()} />;
  } else if (!rows || !totals) {
    body = (
      <div aria-hidden="true" className="flex flex-col gap-6">
        {[0, 1].map((i) => (
          <div key={i} className="flex flex-col gap-2">
            <Skeleton className="h-4 w-28" />
            <Skeleton className="h-7 w-16" />
            <Skeleton className="h-20 w-full" />
          </div>
        ))}
      </div>
    );
  } else if (!hasPerformance(totals)) {
    body = (
      <EmptyState
        icon={GaugeIcon}
        title={t("performance.emptyTitle", { span })}
        body={t("performance.emptyBody")}
        className="flex-1 py-10"
      />
    );
  } else {
    const lookups = totals.hits + totals.misses;
    body = (
      <div
        className={cn(
          "grid gap-6 transition-opacity sm:grid-cols-2 lg:grid-cols-1",
          series.isPlaceholderData && "opacity-60",
        )}
      >
        <Metric
          testId="hit-ratio"
          label={t("performance.ratio")}
          value={lookups === 0 ? null : hitRatioText(totals.hits / lookups)}
          caption={
            lookups === 0
              ? t("performance.noLookups", { span })
              : t("performance.ratioCaption", {
                  hits: number.format(totals.hits),
                  lookups: number.format(lookups),
                })
          }
        >
          {lookups === 0 ? null : (
            <MiniChart
              rows={rows}
              range={range}
              dataKey="hitRatio"
              label={t("performance.ratio")}
              color="var(--chart-2)"
              domain={[0, 100]}
              ticks={[0, 50, 100]}
              tick={(v) => hitRatioText(v / 100)}
              value={(v) => hitRatioText(v / 100)}
            />
          )}
        </Metric>
        <Metric
          testId="page-p95"
          label={t("performance.p95")}
          value={totals.p95 === null ? null : p95Text(totals.p95)}
          caption={
            totals.p95 === null ? t("performance.noPages", { span }) : t("performance.p95Caption")
          }
        >
          {totals.p95 === null ? null : (
            <MiniChart
              rows={rows}
              range={range}
              dataKey="p95"
              label={t("performance.p95")}
              color="var(--chart-5)"
              domain={[0, "dataMax"]}
              tick={durationText}
              value={p95Text}
              step
            />
          )}
        </Metric>
        {series.isError ? <StaleNote at={series.dataUpdatedAt} /> : null}
      </div>
    );
  }

  return (
    <Panel
      title={t("performance.title")}
      description={t("performance.description", { span })}
      scope={<ScopeBadge>{t("metrics.allNodes")}</ScopeBadge>}
      busy={series.isPending || series.isPlaceholderData}
      className={className}
    >
      {body}
    </Panel>
  );
}

function Metric({
  testId,
  label,
  value,
  caption,
  children,
}: {
  testId: string;
  label: string;
  value: string | null;
  caption: string;
  children: ReactNode;
}) {
  return (
    <div className="flex min-w-0 flex-col gap-2" data-testid={testId}>
      <div className="flex flex-col gap-0.5">
        <h3 className="text-sm font-medium text-muted-foreground">{label}</h3>
        {value === null ? null : (
          <p className="text-2xl leading-tight font-semibold text-foreground tabular-nums">
            {value}
          </p>
        )}
        <p className="text-xs text-muted-foreground tabular-nums">{caption}</p>
      </div>
      {children}
    </div>
  );
}

/** One measure per hour as a line (decorative: the headline and caption say it in text). */
function MiniChart({
  rows,
  range,
  dataKey,
  label,
  color,
  domain,
  ticks,
  tick,
  value,
  step = false,
}: {
  rows: MetricPoint[];
  range: MetricsRange;
  dataKey: "hitRatio" | "p95";
  label: string;
  color: string;
  domain: [number, number | "dataMax"];
  ticks?: number[];
  tick: (v: number) => string;
  value: (v: number) => string;
  /** A step line: the p95 is a bucket bound, not a measurement between hours. */
  step?: boolean;
}) {
  const { t } = useTranslation("dashboard");
  const config = { [dataKey]: { label, color } } satisfies ChartConfig;
  const format = tickFormat(range);
  return (
    <ChartContainer config={config} className="aspect-auto h-24 w-full" aria-hidden="true">
      <LineChart data={rows} margin={MARGIN} accessibilityLayer={false}>
        <CartesianGrid vertical={false} />
        <XAxis
          dataKey="t"
          tickLine={false}
          axisLine={false}
          tickMargin={6}
          tickFormatter={format}
          {...(range === "7d"
            ? { ticks: dayTicks(rows), interval: "preserveStartEnd" as const, minTickGap: 8 }
            : { minTickGap: 32 })}
        />
        <YAxis
          tickLine={false}
          axisLine={false}
          tickMargin={4}
          width={Y_WIDTH}
          domain={domain}
          tickCount={3}
          tickFormatter={tick}
          {...(ticks ? { ticks } : {})}
        />
        <ChartTooltip
          cursor
          content={
            <ChartTooltipContent
              indicator="line"
              labelFormatter={bucketTooltipLabel(t("metrics.soFar"))}
              formatter={(v) => (
                <div className="flex w-full items-center justify-between gap-4">
                  <span className="text-muted-foreground">{label}</span>
                  <span className="font-mono font-medium text-foreground tabular-nums">
                    {typeof v === "number" ? value(v) : String(v)}
                  </span>
                </div>
              )}
            />
          }
        />
        <Line
          dataKey={dataKey}
          type={step ? "stepAfter" : "monotone"}
          stroke={`var(--color-${dataKey})`}
          strokeWidth={2}
          // A measured hour between two gaps has no segment to draw: it gets a marker.
          dot={(props: DotItemDotProps) => {
            const i = props.index ?? -1;
            const at = (j: number) => rows[j]?.[dataKey] ?? null;
            const isolated = at(i) !== null && at(i - 1) === null && at(i + 1) === null;
            return isolated && props.cx !== undefined && props.cy !== undefined ? (
              <circle
                key={props.index}
                cx={props.cx}
                cy={props.cy}
                r={4}
                fill={`var(--color-${dataKey})`}
                stroke="var(--card)"
                strokeWidth={2}
              />
            ) : (
              <g key={props.index} />
            );
          }}
          activeDot={{ r: 4, strokeWidth: 2, stroke: "var(--card)" }}
          connectNulls={false}
          isAnimationActive={false}
        />
      </LineChart>
    </ChartContainer>
  );
}
