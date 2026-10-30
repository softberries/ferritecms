// Traffic (`manage_system`, P36): page views and requests per hour as two areas, the
// 5xx errors as their own bar strip under them (a count of a different size on one axis
// would lie flat; red beside the ember page views fails the colour check), and the
// range's totals above, which double as the legend: each names its series' line key
// (solid, dashed) or bar. Every node's figures, summed ("All nodes"). The numbers are
// also in a visually hidden table, per hour over 24 hours and per day over 7.

import { useQuery } from "@tanstack/react-query";
import { cn } from "cn";
import { ActivityIcon, CircleCheckIcon } from "lucide-react";
import { type ReactNode, useMemo } from "react";
import { useTranslation } from "react-i18next";
import { Area, AreaChart, Bar, BarChart, CartesianGrid, XAxis, YAxis } from "recharts";
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
  type MetricTotals,
  byLocalDay,
  dayLabel,
  dayTicks,
  hasTraffic,
  hourLabel,
  points,
  summarise,
  tickFormat,
} from "./metrics-series";
import { Panel, PanelError, ScopeBadge, StaleNote } from "./panel";

const SERIES = [
  { key: "requests", color: "var(--chart-2)", dash: "5 4" },
  { key: "pageViews", color: "var(--chart-1)", dash: undefined },
] as const;
const ERROR_COLOUR = "var(--destructive)";
/** One y-axis width for both charts, so their hours line up. */
const Y_WIDTH = 40;
const MARGIN = { top: 8, right: 4, bottom: 0, left: 0 };

export function TrafficCard({ range, className }: { range: MetricsRange; className?: string }) {
  const { t } = useTranslation("dashboard");
  const series = useQuery(metricsSeriesQuery(range));
  const span = t(`metrics.span.${range}`);
  const data = series.data;
  const rows = useMemo(() => (data ? points(data.buckets) : null), [data]);
  const totals = useMemo(() => (data ? summarise(data.buckets) : null), [data]);

  let body;
  if (series.isError && !data) {
    body = <PanelError error={series.error} retry={() => void series.refetch()} />;
  } else if (!rows || !totals) {
    body = <TrafficSkeleton />;
  } else if (!hasTraffic(totals)) {
    body = (
      <EmptyState
        icon={ActivityIcon}
        title={t("traffic.emptyTitle", { span })}
        body={t("traffic.emptyBody")}
        className="flex-1 py-10"
      />
    );
  } else {
    body = (
      <div
        className={cn(
          "flex flex-col gap-4 transition-opacity",
          series.isPlaceholderData && "opacity-60",
        )}
      >
        <Totals totals={totals} />
        <TrafficChart rows={rows} range={range} />
        <ErrorsStrip rows={rows} range={range} total={totals.errors} span={span} />
        <TrafficTable rows={rows} range={range} span={span} />
        {series.isError ? <StaleNote at={series.dataUpdatedAt} /> : null}
      </div>
    );
  }

  return (
    <Panel
      title={t("traffic.title")}
      description={t("traffic.description", { span })}
      scope={<ScopeBadge>{t("metrics.allNodes")}</ScopeBadge>}
      busy={series.isPending || series.isPlaceholderData}
      className={className}
    >
      {body}
    </Panel>
  );
}

/** A series key as drawn: a short line (solid or dashed) or a bar. */
function Key({ color, dash, bar = false }: { color: string; dash?: string; bar?: boolean }) {
  return bar ? (
    <span
      aria-hidden="true"
      className="h-2.5 w-2 rounded-[2px]"
      style={{ backgroundColor: color }}
    />
  ) : (
    <svg aria-hidden="true" width="20" height="8" viewBox="0 0 20 8" className="shrink-0">
      <line
        x1="1"
        y1="4"
        x2="19"
        y2="4"
        stroke={color}
        strokeWidth="2"
        strokeDasharray={dash}
        strokeLinecap="round"
      />
    </svg>
  );
}

function Totals({ totals }: { totals: MetricTotals }) {
  const { t } = useTranslation("dashboard");
  const number = new Intl.NumberFormat(uiLocale());
  const percent = new Intl.NumberFormat(uiLocale(), {
    style: "percent",
    maximumFractionDigits: 1,
  });
  const rate = totals.requests === 0 ? null : totals.errors / totals.requests;
  return (
    <ul aria-label={t("traffic.totals")} className="grid grid-cols-3 gap-4">
      <Total label={t("traffic.pageViews")} value={number.format(totals.pageViews)}>
        <Key color={SERIES[1].color} />
      </Total>
      <Total label={t("traffic.requests")} value={number.format(totals.requests)}>
        <Key color={SERIES[0].color} dash={SERIES[0].dash} />
      </Total>
      <Total
        label={t("traffic.errors")}
        value={number.format(totals.errors)}
        caption={rate === null ? undefined : t("traffic.errorRate", { rate: percent.format(rate) })}
      >
        <Key color={ERROR_COLOUR} bar />
      </Total>
    </ul>
  );
}

function Total({
  label,
  value,
  caption,
  children,
}: {
  label: string;
  value: string;
  caption?: string | undefined;
  children: ReactNode;
}) {
  return (
    <li className="flex min-w-0 flex-col gap-1">
      {/* The label wraps on a phone ("Page / views") rather than being cut. */}
      <span className="flex items-center gap-2 text-sm leading-tight text-muted-foreground">
        {children}
        <span className="min-w-0">{label}</span>
      </span>
      <span className="text-xl leading-tight sm:text-2xl font-semibold text-foreground tabular-nums">
        {value}
      </span>
      {caption ? (
        <span className="text-xs text-muted-foreground tabular-nums">{caption}</span>
      ) : null}
    </li>
  );
}

/** The x axis of both charts: hours over 24 hours, local midnights over 7 days. */
function xAxisProps(rows: readonly MetricPoint[], range: MetricsRange) {
  return {
    dataKey: "t",
    tickLine: false,
    axisLine: false,
    tickMargin: 8,
    tickFormatter: tickFormat(range),
    ...(range === "7d" ? { ticks: dayTicks(rows), interval: 0 as const } : { minTickGap: 24 }),
  };
}

function compact(value: number): string {
  return new Intl.NumberFormat(uiLocale(), {
    notation: "compact",
    maximumFractionDigits: 1,
  }).format(value);
}

function TrafficChart({ rows, range }: { rows: MetricPoint[]; range: MetricsRange }) {
  const { t } = useTranslation("dashboard");
  const config = {
    requests: { label: t("traffic.requests"), color: SERIES[0].color },
    pageViews: { label: t("traffic.pageViews"), color: SERIES[1].color },
  } satisfies ChartConfig;
  return (
    <ChartContainer
      config={config}
      className="aspect-auto h-52 w-full sm:h-64"
      data-testid="traffic-chart"
    >
      <AreaChart data={rows} margin={MARGIN}>
        <defs>
          {SERIES.map((s) => (
            <linearGradient key={s.key} id={`traffic-${s.key}`} x1="0" y1="0" x2="0" y2="1">
              <stop offset="0%" stopColor={`var(--color-${s.key})`} stopOpacity={0.16} />
              <stop offset="100%" stopColor={`var(--color-${s.key})`} stopOpacity={0.02} />
            </linearGradient>
          ))}
        </defs>
        <CartesianGrid vertical={false} />
        <XAxis {...xAxisProps(rows, range)} />
        <YAxis
          allowDecimals={false}
          tickLine={false}
          axisLine={false}
          tickMargin={4}
          width={Y_WIDTH}
          // Four ticks land on round steps (0, 500, 1K…), five on steps like 350.
          tickCount={4}
          tickFormatter={compact}
        />
        <ChartTooltip
          cursor
          content={
            <ChartTooltipContent
              indicator="line"
              labelFormatter={(_, payload) => {
                const at = (payload[0]?.payload as MetricPoint | undefined)?.t;
                return at === undefined ? null : hourLabel(at);
              }}
            />
          }
        />
        {SERIES.map((s) => (
          <Area
            key={s.key}
            dataKey={s.key}
            type="monotone"
            stroke={`var(--color-${s.key})`}
            strokeWidth={2}
            strokeDasharray={s.dash}
            fill={`url(#traffic-${s.key})`}
            dot={false}
            activeDot={{ r: 4, strokeWidth: 2, stroke: "var(--card)" }}
            isAnimationActive={false}
          />
        ))}
      </AreaChart>
    </ChartContainer>
  );
}

function ErrorsStrip({
  rows,
  range,
  total,
  span,
}: {
  rows: MetricPoint[];
  range: MetricsRange;
  total: number;
  span: string;
}) {
  const { t } = useTranslation("dashboard");
  if (total === 0) {
    return (
      <p className="flex items-center gap-2 text-sm text-muted-foreground">
        <CircleCheckIcon aria-hidden="true" className="size-4 shrink-0 text-success" />
        {t("traffic.noErrors", { span })}
      </p>
    );
  }
  const config = {
    errors: { label: t("traffic.errors"), color: ERROR_COLOUR },
  } satisfies ChartConfig;
  return (
    <div className="flex flex-col gap-1">
      <p className="text-xs font-medium text-muted-foreground">{t("traffic.errorsChart")}</p>
      <ChartContainer
        config={config}
        className="aspect-auto h-16 w-full"
        data-testid="errors-chart"
      >
        <BarChart data={rows} margin={MARGIN} barCategoryGap="20%">
          <XAxis {...xAxisProps(rows, range)} hide />
          <YAxis
            allowDecimals={false}
            tickLine={false}
            axisLine={false}
            tickMargin={4}
            width={Y_WIDTH}
            tickCount={2}
            domain={[0, "dataMax"]}
            tickFormatter={compact}
          />
          <ChartTooltip
            cursor
            content={
              <ChartTooltipContent
                indicator="line"
                labelFormatter={(_, payload) => {
                  const at = (payload[0]?.payload as MetricPoint | undefined)?.t;
                  return at === undefined ? null : hourLabel(at);
                }}
              />
            }
          />
          <Bar
            dataKey="errors"
            fill="var(--color-errors)"
            radius={[2, 2, 0, 0]}
            maxBarSize={12}
            isAnimationActive={false}
          />
        </BarChart>
      </ChartContainer>
    </div>
  );
}

/** The chart's text twin: per hour over 24 hours, per local day over 7. */
function TrafficTable({
  rows,
  range,
  span,
}: {
  rows: MetricPoint[];
  range: MetricsRange;
  span: string;
}) {
  const { t } = useTranslation("dashboard");
  const number = new Intl.NumberFormat(uiLocale());
  const lines =
    range === "24h"
      ? rows.map((r) => ({ key: r.t, label: hourLabel(r.t), ...r }))
      : byLocalDay(rows).map((d) => ({ key: d.day, label: dayLabel(d.day), ...d }));
  return (
    // The wrapper clips: `sr-only` on a table itself widens the page (activity chart).
    <div className="sr-only">
      <table>
        <caption>
          {t(range === "24h" ? "traffic.tableCaptionHours" : "traffic.tableCaptionDays", { span })}
        </caption>
        <thead>
          <tr>
            <th scope="col">{t(range === "24h" ? "traffic.hour" : "traffic.day")}</th>
            <th scope="col">{t("traffic.pageViews")}</th>
            <th scope="col">{t("traffic.requests")}</th>
            <th scope="col">{t("traffic.errors")}</th>
          </tr>
        </thead>
        <tbody>
          {lines.map((l) => (
            <tr key={l.key}>
              <th scope="row">{l.label}</th>
              <td>{number.format(l.pageViews)}</td>
              <td>{number.format(l.requests)}</td>
              <td>{number.format(l.errors)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function TrafficSkeleton() {
  return (
    <div aria-hidden="true" className="flex flex-col gap-4">
      <div className="grid grid-cols-3 gap-4">
        {Array.from({ length: 3 }, (_, i) => (
          <div key={i} className="flex flex-col gap-2">
            <Skeleton className="h-4 w-20" />
            <Skeleton className="h-7 w-16" />
          </div>
        ))}
      </div>
      <Skeleton className="h-52 w-full rounded-lg sm:h-64" />
    </div>
  );
}
