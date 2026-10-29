// The runs table (P32): every import, newest first (CLI runs too), 20 a page with Load
// more. Started, source, state, step, the created/updated/kept/skipped totals and the
// report: the page in a new tab, the JSON as a download. Below 1024 px the totals and the
// step fold into the source cell.

import { useInfiniteQuery } from "@tanstack/react-query";
import { cn } from "cn";
import { DownloadIcon, ExternalLinkIcon, HistoryIcon } from "lucide-react";
import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import { type ImportRunDto, importRunsQuery, reportUrls } from "@/api/import";
import { DataTable } from "@/components/kit/data-table";
import { type DataTableColumn, dataTableColumns } from "@/components/kit/data-table-features";
import { EmptyState } from "@/components/kit/empty-state";
import { ErrorState } from "@/components/kit/error-state";
import { RelativeTime } from "@/components/kit/relative-time";
import { Button } from "@/components/ui/button";
import { uiLocale } from "@/lib/locale";
import { countTotals } from "./import-model";
import { DryRunBadge } from "./run-panel";

const RUN_STATES = ["running", "done", "failed", "abandoned"] as const;
type RunState = (typeof RUN_STATES)[number];
const STATE_TONE: Record<RunState, string> = {
  running: "text-info",
  done: "text-success",
  failed: "text-destructive",
  abandoned: "text-warning",
};
const TOTAL_COLUMNS = ["created", "updated", "kept", "skipped"] as const;

export function RunsTable() {
  const { t } = useTranslation("import");
  const list = useInfiniteQuery(importRunsQuery);
  const runs = useMemo(() => {
    const seen = new Map<string, ImportRunDto>();
    for (const page of list.data?.pages ?? []) {
      for (const r of page.items) if (!seen.has(r.id)) seen.set(r.id, r);
    }
    return [...seen.values()];
  }, [list.data]);
  const columns = useColumns();

  return (
    <section
      data-slot="import-runs"
      aria-labelledby="import-runs-title"
      className="flex min-w-0 flex-col gap-3"
    >
      <h2 id="import-runs-title" className="text-md font-medium text-foreground">
        {t("runs.title")}
      </h2>
      {list.isError && !list.data ? (
        <ErrorState error={list.error} retry={() => void list.refetch()} />
      ) : (
        <DataTable
          label={t("runs.label")}
          columns={columns}
          data={runs}
          getRowId={(r) => r.id}
          loading={list.isPending}
          openOnClick={false}
          columnClassName={{
            started: "py-3 whitespace-nowrap",
            source: "w-full max-w-0 py-2.5",
            state: "hidden py-3 sm:table-cell",
            step: "hidden py-3 whitespace-nowrap lg:table-cell",
            created: "hidden py-3 text-right lg:table-cell",
            updated: "hidden py-3 text-right lg:table-cell",
            kept: "hidden py-3 text-right lg:table-cell",
            skipped: "hidden py-3 text-right lg:table-cell",
            report: "py-2 whitespace-nowrap",
          }}
          empty={
            <EmptyState
              icon={HistoryIcon}
              title={t("runs.empty.title")}
              body={t("runs.empty.body")}
              className="border-0"
            />
          }
          footer={
            list.hasNextPage ? (
              <div className="flex justify-center border-t px-4 py-3">
                <Button
                  variant="ghost"
                  disabled={list.isFetchingNextPage}
                  aria-busy={list.isFetchingNextPage || undefined}
                  onClick={() => {
                    if (!list.isFetchingNextPage) void list.fetchNextPage();
                  }}
                >
                  {list.isFetchingNextPage ? t("runs.loadingMore") : t("runs.loadMore")}
                </Button>
              </div>
            ) : null
          }
        />
      )}
    </section>
  );
}

/** A run state as a dot plus a label, never colour alone. */
export function RunStateBadge({ state }: { state: string }) {
  const { t } = useTranslation("import");
  const known = (RUN_STATES as readonly string[]).includes(state);
  const s = state as RunState;
  return (
    <span
      data-slot="run-state"
      data-state={state}
      className={cn(
        "inline-flex h-6 shrink-0 items-center gap-1.5 rounded-full border px-2 text-xs font-medium whitespace-nowrap",
        known ? STATE_TONE[s] : "text-muted-foreground",
      )}
    >
      <span
        aria-hidden="true"
        className={cn(
          "size-2 shrink-0 rounded-full border-[1.5px] border-current",
          state === "running" ? "bg-transparent" : "bg-current",
        )}
      />
      {known ? t(`runs.status.${s}`) : state}
    </span>
  );
}

function useColumns(): DataTableColumn<ImportRunDto>[] {
  const { t } = useTranslation("import");
  return useMemo(() => {
    const col = dataTableColumns<ImportRunDto>();
    const number = new Intl.NumberFormat(uiLocale());
    const none = <span className="text-muted-foreground">{t("runs.none")}</span>;
    const stepLabel = (step: string | null | undefined) =>
      step ? t(`run.step.${step as "parse"}`, { defaultValue: step }) : null;
    return [
      col.accessor("started_at", {
        id: "started",
        header: () => t("runs.started"),
        cell: (c) => (
          <RelativeTime at={c.row.original.started_at} className="text-sm text-foreground" />
        ),
      }),
      col.accessor("source", {
        id: "source",
        header: () => t("runs.source"),
        cell: (c) => {
          const r = c.row.original;
          const totals = countTotals(r.counts);
          return (
            <div className="flex min-w-0 flex-col gap-1">
              <div className="flex min-w-0 items-center gap-2">
                <span
                  className="min-w-0 truncate font-mono text-sm text-foreground"
                  title={r.source}
                >
                  {r.source}
                </span>
                {r.dry_run ? <DryRunBadge /> : null}
              </div>
              <span
                data-slot="run-summary"
                className="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs whitespace-normal text-muted-foreground lg:hidden"
              >
                <span className="sm:hidden">
                  <RunStateBadge state={r.state} />
                </span>
                {r.step ? <span>{stepLabel(r.step)}</span> : null}
                <span className="tabular-nums">
                  {t("runs.summary", {
                    created: number.format(totals.created),
                    updated: number.format(totals.updated),
                    kept: number.format(totals.kept),
                    skipped: number.format(totals.skipped),
                  })}
                </span>
              </span>
              {r.error ? (
                <span
                  className="line-clamp-2 text-xs wrap-anywhere text-destructive"
                  title={r.error}
                >
                  {r.error}
                </span>
              ) : null}
            </div>
          );
        },
      }),
      col.accessor("state", {
        id: "state",
        header: () => t("runs.state"),
        cell: (c) => <RunStateBadge state={c.row.original.state} />,
      }),
      col.accessor("step", {
        id: "step",
        header: () => t("runs.step"),
        cell: (c) => {
          const label = stepLabel(c.row.original.step);
          return label ? <span className="text-sm text-muted-foreground">{label}</span> : none;
        },
      }),
      ...TOTAL_COLUMNS.map((k) =>
        col.display({
          id: k,
          header: () => <span className="block text-right">{t(`columns.${k}`)}</span>,
          cell: (c) => {
            const n = countTotals(c.row.original.counts)[k];
            return (
              <span
                className={cn(
                  "block text-right text-sm tabular-nums",
                  n === 0 ? "text-muted-foreground" : "text-foreground",
                )}
              >
                {number.format(n)}
              </span>
            );
          },
        }),
      ),
      col.display({
        id: "report",
        header: () => t("runs.report"),
        cell: (c) => {
          const r = c.row.original;
          if (!r.report) return none;
          const urls = reportUrls(r.id);
          return (
            <div className="flex items-center gap-1">
              <Button asChild variant="ghost" size="sm" className="pointer-coarse:h-11">
                <a
                  href={urls.html}
                  target="_blank"
                  rel="noopener noreferrer"
                  aria-label={t("runs.htmlOn", { source: r.source })}
                >
                  {t("runs.html")}
                  <ExternalLinkIcon data-icon="inline-end" />
                </a>
              </Button>
              <Button asChild variant="ghost" size="sm" className="pointer-coarse:h-11">
                <a
                  href={urls.json}
                  download={`import-${r.id}.json`}
                  aria-label={t("runs.jsonOn", { source: r.source })}
                >
                  <DownloadIcon data-icon="inline-start" />
                  {t("runs.json")}
                </a>
              </Button>
            </div>
          );
        },
      }),
    ];
  }, [t]);
}
