// Kit patterns (page header, states, metric tiles), the data table and charts.

import { FileTextIcon, PencilIcon, PlusIcon, Trash2Icon, UploadIcon } from "lucide-react";
import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { CartesianGrid, Line, LineChart, XAxis, YAxis } from "recharts";
import { toast } from "sonner";
import type { ContentStatus } from "@/api/content";
import { ApiError } from "@/api/problem";
import { DataTable } from "@/components/kit/data-table";
import { dataTableColumns, type RowSelectionState } from "@/components/kit/data-table-features";
import { EmptyState } from "@/components/kit/empty-state";
import { ErrorState } from "@/components/kit/error-state";
import { ForbiddenState } from "@/components/kit/forbidden-state";
import { KbdCombo } from "@/components/kit/kbd-combo";
import { MetricTile } from "@/components/kit/metric-tile";
import { PageHeader } from "@/components/kit/page-header";
import { RelativeTime } from "@/components/kit/relative-time";
import { SaveState } from "@/components/kit/save-state";
import { Sparkline } from "@/components/kit/sparkline";
import { StatusBadge } from "@/components/kit/status-badge";
import { Button } from "@/components/ui/button";
import { uiLocale } from "@/lib/locale";
import {
  ChartContainer,
  ChartTooltip,
  ChartTooltipContent,
  type ChartConfig,
} from "@/components/ui/chart";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { Block, Section, Specimen } from "./frame";

/** A deterministic wave, so screenshots are stable. */
const series = (n: number, seed: number, base: number, amp: number) =>
  Array.from({ length: n }, (_, i) =>
    Math.round(base + amp * Math.sin((i + seed) / 2.3) + (amp / 2) * Math.cos((i * seed) / 5)),
  );

const busy = new ApiError({
  type: "/api/v1/problems/busy",
  title: "Service busy",
  status: 503,
  detail: "The server is busy. Try again in 5 s.",
});

export function PatternsSection() {
  const { t } = useTranslation(["styleguide", "common"]);
  return (
    <Section id="patterns">
      <Block title={t("patterns.header.title")} description={t("patterns.header.body")}>
        <Specimen>
          <PageHeader
            title={t("navigation.items.posts")}
            description={t("patterns.header.description")}
            actions={
              <>
                <Button variant="outline">
                  <UploadIcon data-icon="inline-start" />
                  {t("patterns.header.import")}
                </Button>
                <Button>
                  <PlusIcon data-icon="inline-start" />
                  {t("controls.sizes.newPost")}
                </Button>
              </>
            }
            tabs={
              <Tabs defaultValue="all">
                <TabsList variant="line">
                  {(
                    [
                      ["all", 128],
                      ["published", 96],
                      ["draft", 24],
                      ["scheduled", 8],
                    ] as const
                  ).map(([key, n]) => (
                    <TabsTrigger key={key} value={key} className="gap-1.5">
                      {key === "all" ? t("controls.tabs.all") : t(`common:status.${key}`)}
                      <span className="text-xs text-muted-foreground tabular-nums">{n}</span>
                    </TabsTrigger>
                  ))}
                </TabsList>
                {(["all", "published", "draft", "scheduled"] as const).map((key) => (
                  <TabsContent key={key} value={key} className="pt-3 text-sm text-muted-foreground">
                    {t("controls.tabs.panel")}
                  </TabsContent>
                ))}
              </Tabs>
            }
          />
        </Specimen>
      </Block>

      <Block title={t("patterns.metrics.title")} description={t("patterns.metrics.body")}>
        <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-4">
          <MetricTile
            label={t("patterns.metrics.published")}
            value={1284}
            delta={12}
            series={series(30, 1, 40, 8)}
            href="/"
          />
          <MetricTile
            label={t("patterns.metrics.drafts")}
            value={37}
            delta={-3}
            series={series(30, 3, 30, 6)}
          />
          <MetricTile label={t("patterns.metrics.pending")} value={5} delta={0} />
          <MetricTile label={t("patterns.metrics.scheduled")} value={8} />
        </div>
      </Block>

      <div className="grid gap-6 lg:grid-cols-3">
        <Block title={t("patterns.empty.title")} description={t("patterns.empty.body")}>
          <EmptyState
            icon={FileTextIcon}
            title={t("patterns.empty.heading")}
            body={t("patterns.empty.text")}
            action={
              <Button>
                <PlusIcon data-icon="inline-start" />
                {t("controls.sizes.newPost")}
              </Button>
            }
            className="h-full"
          />
        </Block>
        <Block title={t("patterns.error.title")} description={t("patterns.error.body")}>
          <ErrorState error={busy} retry={() => undefined} className="h-full" />
        </Block>
        <Block title={t("patterns.forbidden.title")} description={t("patterns.forbidden.body")}>
          <ForbiddenState className="h-full" />
        </Block>
      </div>

      <Block title={t("patterns.editorBar.title")} description={t("patterns.editorBar.body")}>
        <Specimen className="flex flex-wrap items-center justify-between gap-3 py-3">
          <div className="flex min-w-0 items-center gap-3">
            <StatusBadge status="draft" />
            <SaveState state="saved" at={new Date(2026, 9, 25, 14, 2)} />
          </div>
          <div className="flex items-center gap-2">
            <Button variant="ghost">{t("patterns.editorBar.preview")}</Button>
            <Tooltip>
              <TooltipTrigger asChild>
                <Button>{t("patterns.editorBar.publish")}</Button>
              </TooltipTrigger>
              <TooltipContent>
                <KbdCombo keys="mod+shift+p" />
              </TooltipContent>
            </Tooltip>
          </div>
        </Specimen>
      </Block>
    </Section>
  );
}

interface Post {
  id: SampleId;
  title: string;
  status: ContentStatus;
  author: string;
  words: number;
  updated: string;
}

const now = Date.now();
const ago = (minutes: number) => new Date(now - minutes * 60_000).toISOString();

type SampleId = "p1" | "p2" | "p3" | "p4" | "p5" | "p6";

/** Sample rows; titles come from the styleguide strings. */
const SAMPLES: Omit<Post, "title">[] = [
  { id: "p1", status: "published", author: "Ada Lovelace", words: 1840, updated: ago(12) },
  { id: "p2", status: "scheduled", author: "Grace Hopper", words: 2210, updated: ago(95) },
  { id: "p3", status: "pending", author: "Katherine Johnson", words: 960, updated: ago(60 * 7) },
  { id: "p4", status: "draft", author: "Ada Lovelace", words: 312, updated: ago(60 * 26) },
  { id: "p5", status: "private", author: "Radia Perlman", words: 4120, updated: ago(60 * 24 * 9) },
  { id: "p6", status: "trash", author: "Grace Hopper", words: 140, updated: ago(60 * 24 * 40) },
];

export function DataSection() {
  const { t } = useTranslation("styleguide");
  const [selection, setSelection] = useState<RowSelectionState>({ p2: true });
  const posts = useMemo<Post[]>(
    () => SAMPLES.map((p) => ({ ...p, title: t(`data.samples.${p.id}`) })),
    [t],
  );
  const columns = useMemo(() => {
    const col = dataTableColumns<Post>();
    return col.columns([
      col.accessor("title", {
        header: () => t("data.columns.title"),
        cell: (c) => (
          <span
            className="block max-w-44 truncate font-medium sm:max-w-72 lg:max-w-md"
            title={c.getValue()}
          >
            {c.getValue()}
          </span>
        ),
      }),
      col.accessor("status", {
        header: () => t("data.columns.status"),
        cell: (c) => <StatusBadge status={c.getValue()} />,
      }),
      col.accessor("author", {
        header: () => t("data.columns.author"),
        cell: (c) => <span className="text-muted-foreground">{c.getValue()}</span>,
      }),
      col.accessor("words", {
        header: () => <span className="block text-right">{t("data.columns.words")}</span>,
        cell: (c) => (
          <span className="block text-right tabular-nums">
            {c.getValue().toLocaleString(uiLocale())}
          </span>
        ),
      }),
      col.accessor("updated", {
        header: () => t("data.columns.updated"),
        cell: (c) => <RelativeTime at={c.getValue()} className="text-muted-foreground" />,
      }),
    ]);
  }, [t]);
  const hide = {
    author: "hidden md:table-cell",
    words: "hidden lg:table-cell",
    updated: "hidden sm:table-cell",
  };
  // The half-width specimens show fewer columns.
  const narrow = { ...hide, author: "hidden 2xl:table-cell", words: "hidden 2xl:table-cell" };
  const count = Object.keys(selection).length;
  return (
    <Section id="data">
      <Block
        title={t("data.table.title")}
        description={
          <>
            {t("data.table.body")}{" "}
            <span className="inline-flex flex-wrap items-center gap-1 align-middle">
              <KbdCombo keys="j" /> <KbdCombo keys="k" /> <KbdCombo keys="x" />{" "}
              <KbdCombo keys="enter" />
            </span>
          </>
        }
      >
        <div className="flex h-8 items-center justify-between gap-3 text-sm">
          <span className="text-muted-foreground tabular-nums" aria-live="polite">
            {t("data.table.selected", { count })}
          </span>
          {count > 0 ? (
            <Button variant="ghost" size="sm" onClick={() => setSelection({})}>
              {t("data.table.clear")}
            </Button>
          ) : null}
        </div>
        <DataTable
          label={t("data.table.title")}
          columns={columns}
          data={posts}
          getRowId={(p) => p.id}
          selection={{ state: selection, onChange: setSelection }}
          onRowOpen={(p) => toast(t("data.table.opened", { title: p.title }))}
          loading={false}
          empty={null}
          columnClassName={hide}
          rowActions={(p) => (
            <>
              <Tooltip>
                <TooltipTrigger asChild>
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    aria-label={t("data.table.edit", { title: p.title })}
                  >
                    <PencilIcon />
                  </Button>
                </TooltipTrigger>
                <TooltipContent>{t("overlays.menu.edit")}</TooltipContent>
              </Tooltip>
              <Tooltip>
                <TooltipTrigger asChild>
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    aria-label={t("data.table.trash", { title: p.title })}
                  >
                    <Trash2Icon />
                  </Button>
                </TooltipTrigger>
                <TooltipContent>{t("overlays.menu.trash")}</TooltipContent>
              </Tooltip>
            </>
          )}
        />
      </Block>
      <div className="grid gap-6 lg:grid-cols-2">
        <Block title={t("data.loading.title")} description={t("data.loading.body")}>
          <DataTable
            label={t("data.loading.title")}
            columns={columns}
            data={[]}
            getRowId={(p) => p.id}
            loading
            empty={null}
            columnClassName={narrow}
            className="flex-1"
          />
        </Block>
        <Block title={t("data.empty.title")} description={t("data.empty.body")}>
          <DataTable
            label={t("data.empty.title")}
            columns={columns}
            data={[]}
            getRowId={(p) => p.id}
            loading={false}
            columnClassName={narrow}
            className="flex-1"
            empty={
              <EmptyState
                icon={FileTextIcon}
                title={t("patterns.empty.heading")}
                body={t("data.empty.text")}
                className="border-0 py-8"
              />
            }
          />
        </Block>
      </div>
    </Section>
  );
}

const DAYS = 14;
const CHART_KEYS = ["posts", "pages", "comments", "media", "users"] as const;
/** Series differ by dash pattern as well as colour (11 §3). */
const DASH: Record<(typeof CHART_KEYS)[number], string | undefined> = {
  posts: undefined,
  pages: "6 4",
  comments: "2 3",
  media: "10 3 2 3",
  users: "1 6",
};

export function ChartsSection() {
  const { t } = useTranslation("styleguide");
  const config = useMemo(
    () =>
      Object.fromEntries(
        CHART_KEYS.map((key, i) => [
          key,
          { label: t(`charts.series.${key}`), color: `var(--chart-${i + 1})` },
        ]),
      ) satisfies ChartConfig,
    [t],
  );
  const data = useMemo(() => {
    const values = CHART_KEYS.map((_, i) => series(DAYS, i + 2, 40 - i * 6, 9 - i));
    return Array.from({ length: DAYS }, (_, d) => {
      const date = new Date(2026, 9, 12 + d);
      const row: Record<string, number | string> = {
        day: date.toLocaleDateString(uiLocale(), { month: "short", day: "numeric" }),
      };
      CHART_KEYS.forEach((key, i) => {
        row[key] = Math.max(0, values[i]?.[d] ?? 0);
      });
      return row;
    });
  }, []);
  return (
    <Section id="charts">
      <Block title={t("charts.lines.title")} description={t("charts.lines.body")}>
        <Specimen className="flex flex-col gap-4">
          <ChartContainer config={config} className="aspect-auto h-72 w-full">
            <LineChart data={data} margin={{ top: 8, right: 8, bottom: 0, left: -16 }}>
              <CartesianGrid vertical={false} />
              <XAxis
                dataKey="day"
                tickLine={false}
                axisLine={false}
                tickMargin={8}
                minTickGap={24}
              />
              <YAxis tickLine={false} axisLine={false} tickMargin={4} width={40} />
              <ChartTooltip cursor content={<ChartTooltipContent indicator="line" />} />
              {CHART_KEYS.map((key) => (
                <Line
                  key={key}
                  dataKey={key}
                  type="monotone"
                  stroke={`var(--color-${key})`}
                  strokeWidth={2}
                  strokeDasharray={DASH[key]}
                  dot={false}
                  isAnimationActive={false}
                />
              ))}
            </LineChart>
          </ChartContainer>
          <ul className="flex flex-wrap gap-x-5 gap-y-2 text-sm">
            {CHART_KEYS.map((key, i) => (
              <li key={key} className="flex items-center gap-2">
                <svg aria-hidden="true" width="24" height="8" viewBox="0 0 24 8">
                  <line
                    x1="1"
                    y1="4"
                    x2="23"
                    y2="4"
                    stroke={`var(--chart-${i + 1})`}
                    strokeWidth="2"
                    strokeDasharray={DASH[key]}
                    strokeLinecap="round"
                  />
                </svg>
                {t(`charts.series.${key}`)}
              </li>
            ))}
          </ul>
        </Specimen>
      </Block>
      <Block title={t("charts.spark.title")} description={t("charts.spark.body")}>
        <div className="grid gap-4 sm:grid-cols-3">
          {(["posts", "comments", "media"] as const).map((key, i) => {
            const values = series(30, i + 4, 30, 8);
            return (
              <Specimen key={key} className="flex flex-col gap-2 p-4">
                <span className="flex items-baseline justify-between text-sm">
                  <span className="text-muted-foreground">{t(`charts.series.${key}`)}</span>
                  <span className="font-semibold tabular-nums">{values.at(-1)}</span>
                </span>
                <Sparkline data={values} />
              </Specimen>
            );
          })}
        </div>
      </Block>
    </Section>
  );
}
