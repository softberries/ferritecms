// Publishing activity (every role): posts and pages published per day over the last 30
// days, as two areas. Posts are a solid line, pages a dashed one, and the legend names
// both with their totals, so the series never differ by colour alone (11 §3). The same
// numbers are in a visually hidden table (the chart's text twin).

import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { CalendarRangeIcon, PenLineIcon } from "lucide-react";
import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import { Area, AreaChart, CartesianGrid, XAxis, YAxis } from "recharts";
import { EmptyState } from "@/components/kit/empty-state";
import { Button } from "@/components/ui/button";
import {
  ChartContainer,
  ChartTooltip,
  ChartTooltipContent,
  type ChartConfig,
} from "@/components/ui/chart";
import { Skeleton } from "@/components/ui/skeleton";
import { uiLocale } from "@/lib/locale";
import {
  ACTIVITY_DAYS,
  PUBLISHED_CAP,
  browserTimeZone,
  bucketByDay,
  isCapped,
  publishedQuery,
} from "./activity";
import { Panel, PanelError } from "./panel";

const SERIES = [
  { key: "posts", type: "post", color: "var(--chart-1)", dash: undefined },
  { key: "pages", type: "page", color: "var(--chart-2)", dash: "5 4" },
] as const;

type Row = { day: string; posts: number; pages: number };

/** A `YYYY-MM-DD` day as a short local label ("Oct 3"); the day is a calendar date. */
function dayLabel(day: string, options: Intl.DateTimeFormatOptions): string {
  const [y, m, d] = day.split("-").map(Number);
  return new Date(Date.UTC(y ?? 1970, (m ?? 1) - 1, d ?? 1)).toLocaleDateString(uiLocale(), {
    ...options,
    timeZone: "UTC",
  });
}

export function ActivityChart({ canWrite }: { canWrite: boolean }) {
  const { t } = useTranslation("dashboard");
  const posts = useQuery(publishedQuery("post"));
  const pages = useQuery(publishedQuery("page"));
  const number = new Intl.NumberFormat(uiLocale());

  const config = useMemo(
    () =>
      ({
        posts: { label: t("activity.posts"), color: SERIES[0].color },
        pages: { label: t("activity.pages"), color: SERIES[1].color },
      }) satisfies ChartConfig,
    [t],
  );

  const rows = useMemo<Row[] | null>(() => {
    if (!posts.data || !pages.data) return null;
    const now = new Date();
    const tz = browserTimeZone();
    const p = bucketByDay(posts.data, ACTIVITY_DAYS, now, tz);
    const g = bucketByDay(pages.data, ACTIVITY_DAYS, now, tz);
    return p.map((b, i) => ({ day: b.day, posts: b.count, pages: g[i]?.count ?? 0 }));
  }, [posts.data, pages.data]);

  const totals = rows
    ? {
        posts: rows.reduce((n, r) => n + r.posts, 0),
        pages: rows.reduce((n, r) => n + r.pages, 0),
      }
    : null;
  const capped = (posts.data && isCapped(posts.data)) || (pages.data && isCapped(pages.data));
  const failed = posts.isError ? posts : pages.isError ? pages : null;

  let body;
  if (failed) {
    body = (
      <PanelError
        error={failed.error}
        retry={() => {
          if (posts.isError) void posts.refetch();
          if (pages.isError) void pages.refetch();
        }}
      />
    );
  } else if (!rows || !totals) {
    body = <ChartSkeleton />;
  } else if (totals.posts + totals.pages === 0) {
    body = (
      <EmptyState
        icon={CalendarRangeIcon}
        title={t("activity.emptyTitle", { days: ACTIVITY_DAYS })}
        body={t("activity.emptyBody")}
        className="flex-1 py-8"
        action={
          canWrite ? (
            <Button variant="outline" asChild>
              <Link to={"/content/post/new" as "/"}>
                <PenLineIcon data-icon="inline-start" />
                {t("activity.write")}
              </Link>
            </Button>
          ) : undefined
        }
      />
    );
  } else {
    body = (
      <>
        <ChartContainer
          config={config}
          className="aspect-auto h-60 w-full sm:h-64"
          data-testid="activity-chart"
        >
          <AreaChart data={rows} margin={{ top: 8, right: 4, bottom: 0, left: 0 }}>
            <defs>
              {SERIES.map((s) => (
                <linearGradient key={s.key} id={`activity-${s.key}`} x1="0" y1="0" x2="0" y2="1">
                  <stop offset="0%" stopColor={`var(--color-${s.key})`} stopOpacity={0.16} />
                  <stop offset="100%" stopColor={`var(--color-${s.key})`} stopOpacity={0.02} />
                </linearGradient>
              ))}
            </defs>
            <CartesianGrid vertical={false} />
            <XAxis
              dataKey="day"
              tickLine={false}
              axisLine={false}
              tickMargin={8}
              minTickGap={28}
              tickFormatter={(day: string) => dayLabel(day, { month: "short", day: "numeric" })}
            />
            <YAxis
              allowDecimals={false}
              tickLine={false}
              axisLine={false}
              tickMargin={4}
              width={28}
            />
            <ChartTooltip
              cursor
              content={
                <ChartTooltipContent
                  indicator="line"
                  labelFormatter={(_, payload) => {
                    const day = (payload[0]?.payload as Row | undefined)?.day;
                    return day
                      ? dayLabel(day, { weekday: "short", month: "short", day: "numeric" })
                      : null;
                  }}
                />
              }
            />
            {SERIES.map((s) => (
              <Area
                key={s.key}
                dataKey={s.key}
                // Steps: each day is one discrete count, not a curve between days.
                type="step"
                stroke={`var(--color-${s.key})`}
                strokeWidth={2}
                strokeDasharray={s.dash}
                fill={`url(#activity-${s.key})`}
                dot={false}
                activeDot={{ r: 4, strokeWidth: 2, stroke: "var(--card)" }}
                isAnimationActive={false}
              />
            ))}
          </AreaChart>
        </ChartContainer>
        {/* The wrapper is what clips: a table box ignores `width`/`overflow` and grows to
            its content, so `sr-only` on the table itself widened the page (review). */}
        <div className="sr-only">
          <table>
            <caption>{t("activity.tableCaption", { days: ACTIVITY_DAYS })}</caption>
            <thead>
              <tr>
                <th scope="col">{t("activity.day")}</th>
                <th scope="col">{t("activity.posts")}</th>
                <th scope="col">{t("activity.pages")}</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((r) => (
                <tr key={r.day}>
                  <th scope="row">{dayLabel(r.day, { month: "long", day: "numeric" })}</th>
                  <td>{r.posts}</td>
                  <td>{r.pages}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
        {capped ? (
          <p className="mt-3 text-xs text-muted-foreground">
            {t("activity.capped", {
              count: PUBLISHED_CAP,
              formatted: number.format(PUBLISHED_CAP),
            })}
          </p>
        ) : null}
      </>
    );
  }

  return (
    <Panel
      title={t("activity.title")}
      description={t("activity.description", { days: ACTIVITY_DAYS })}
      busy={!failed && !rows}
      action={totals && totals.posts + totals.pages > 0 ? <Legend totals={totals} /> : null}
      className="lg:col-span-8"
    >
      {body}
    </Panel>
  );
}

/** The legend: each series' line key (solid or dashed), its name and its total. */
function Legend({ totals }: { totals: { posts: number; pages: number } }) {
  const { t } = useTranslation("dashboard");
  const number = new Intl.NumberFormat(uiLocale());
  return (
    <ul className="flex items-center gap-4 text-sm" aria-label={t("activity.legend")}>
      {SERIES.map((s) => (
        <li key={s.key} className="flex items-center gap-2">
          <svg aria-hidden="true" width="20" height="8" viewBox="0 0 20 8">
            <line
              x1="1"
              y1="4"
              x2="19"
              y2="4"
              stroke={s.color}
              strokeWidth="2"
              strokeDasharray={s.dash}
              strokeLinecap="round"
            />
          </svg>
          <span className="text-muted-foreground">{t(`activity.${s.key}`)}</span>
          <span className="font-semibold text-foreground tabular-nums">
            {number.format(totals[s.key])}
          </span>
        </li>
      ))}
    </ul>
  );
}

function ChartSkeleton() {
  return (
    <div aria-hidden="true" className="flex h-60 flex-col justify-end gap-3 sm:h-64">
      <Skeleton className="h-full w-full rounded-lg" />
      <div className="flex justify-between">
        {Array.from({ length: 5 }, (_, i) => (
          <Skeleton key={i} className="h-3 w-10" />
        ))}
      </div>
    </div>
  );
}
