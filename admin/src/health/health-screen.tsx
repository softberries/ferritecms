// Content health (M6-4, design 16 §11.2): the newest run's totals (errors, warnings,
// notices), when it ran and runs next, "Run now"; tabs per category; a table of findings
// (rule, item with type and language, what was found, severity) filtered by type,
// language and severity, each row linking to the item's editor at the finding. Never
// run, running and failed states; with external links off, a note links to Settings.

import { Link } from "@tanstack/react-router";
import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { HeartPulseIcon, PlayIcon } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { isProblem } from "@/api/problem";
import {
  type HealthFilters,
  type HealthSummary,
  healthFindingsQuery,
  healthSummaryQuery,
  requestHealthRun,
} from "@/api/quality";
import { qk } from "@/api/keys";
import { typesQuery } from "@/api/types";
import { EmptyState } from "@/components/kit/empty-state";
import { ErrorState } from "@/components/kit/error-state";
import { PageHeader } from "@/components/kit/page-header";
import { RelativeTime } from "@/components/kit/relative-time";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { messageFor, titleFor } from "@/editor/quality/findings";
import { useSiteLocales } from "@/lib/locales";
import { CATEGORIES, type Tab, findingHref, runState, severityTotals } from "./health-model";

const SEVERITY_FILTERS = ["", "error", "warning", "notice"] as const;

export function HealthScreen() {
  const { t } = useTranslation("quality");
  const summary = useQuery(healthSummaryQuery);
  const [tab, setTab] = useState<Tab>("all");
  const [type, setType] = useState("");
  const [locale, setLocale] = useState("");
  const [severity, setSeverity] = useState("");
  const filters: HealthFilters = {
    ...(tab !== "all" ? { category: tab } : {}),
    ...(type ? { type } : {}),
    ...(locale ? { locale } : {}),
    ...(severity ? { severity } : {}),
  };
  const qc = useQueryClient();
  const run = useMutation({
    mutationFn: requestHealthRun,
    onSuccess: () => {
      toast.success(t("health.queued"));
      void qc.invalidateQueries({ queryKey: qk.health.all });
    },
    onError: (e) => {
      toast.error(isProblem(e, "conflict") ? t("health.alreadyRunning") : t("health.runFailed"));
    },
  });
  const state = runState(summary.data);
  const totals = severityTotals(summary.data);
  return (
    <div className="mx-auto flex w-full max-w-7xl flex-col gap-6" data-slot="health-screen">
      <PageHeader
        title={t("health.title")}
        description={t("health.description")}
        actions={
          <Button
            type="button"
            onClick={() => run.mutate()}
            disabled={run.isPending || state === "running"}
            data-slot="health-run"
          >
            <PlayIcon data-icon="inline-start" />
            {state === "running" ? t("health.running") : t("health.runNow")}
          </Button>
        }
      />
      {summary.isError ? (
        <ErrorState error={summary.error} retry={() => void summary.refetch()} />
      ) : !summary.data ? (
        <Skeleton className="h-28 w-full" />
      ) : (
        <SummaryTiles totals={totals} summary={summary.data} />
      )}
      {summary.data && !summary.data.external_links ? (
        <p className="text-sm text-muted-foreground" data-slot="health-external-off">
          {t("health.externalOff")}{" "}
          <Link to="/settings/$group" params={{ group: "health" }} className="underline">
            {t("health.externalSettings")}
          </Link>
        </p>
      ) : null}
      {state === "never" && summary.data ? (
        <EmptyState
          icon={HeartPulseIcon}
          title={t("health.neverTitle")}
          body={t("health.neverBody")}
        />
      ) : (
        <>
          <Tabs value={tab} onValueChange={(v) => setTab(v as Tab)}>
            <TabsList aria-label={t("health.tabsLabel")}>
              <TabsTrigger value="all" className="min-h-11">
                {t("health.all")}
              </TabsTrigger>
              {CATEGORIES.map((c) => (
                <TabsTrigger key={c} value={c} className="min-h-11">
                  {t(`category.${c}`)}
                </TabsTrigger>
              ))}
            </TabsList>
            {/* One panel for the selected tab, so its trigger's `aria-controls` names it. */}
            <TabsContent value={tab} className="flex flex-col gap-4">
              <Filters
                type={type}
                setType={setType}
                locale={locale}
                setLocale={setLocale}
                severity={severity}
                setSeverity={setSeverity}
              />
              <FindingsTable filters={filters} />
            </TabsContent>
          </Tabs>
        </>
      )}
    </div>
  );
}

function SummaryTiles({
  totals,
  summary,
}: {
  totals: { error: number; warning: number; notice: number };
  summary: HealthSummary;
}) {
  const { t } = useTranslation("quality");
  return (
    <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-4" data-slot="health-summary">
      {(["error", "warning", "notice"] as const).map((s) => (
        <Card key={s}>
          <CardContent className="flex flex-col gap-1">
            <span className="text-sm text-muted-foreground">{t(`severity.${s}`)}</span>
            <span className="text-2xl font-semibold tabular-nums" data-slot={`health-${s}`}>
              {totals[s]}
            </span>
          </CardContent>
        </Card>
      ))}
      <Card>
        <CardContent className="flex flex-col gap-1 text-sm">
          <span className="text-muted-foreground">{t("health.lastRun")}</span>
          {summary.last?.finished_at ? (
            <RelativeTime at={summary.last.finished_at} />
          ) : (
            <span>{t("health.notYet")}</span>
          )}
          {summary.running ? <span>{t("health.running")}</span> : null}
          <span className="text-muted-foreground">
            {t("health.nextRun")} <RelativeTime at={summary.next_at} />
          </span>
          {summary.last?.truncated ? (
            <span className="text-muted-foreground">{t("health.truncated")}</span>
          ) : null}
        </CardContent>
      </Card>
    </div>
  );
}

function Filters(props: {
  type: string;
  setType: (v: string) => void;
  locale: string;
  setLocale: (v: string) => void;
  severity: string;
  setSeverity: (v: string) => void;
}) {
  const { t } = useTranslation("quality");
  const types = useQuery(typesQuery);
  const locales = useSiteLocales();
  const select =
    "h-11 rounded-md border border-input bg-background px-3 text-sm focus-visible:ring-2 focus-visible:ring-ring";
  return (
    <div className="flex flex-wrap gap-3" data-slot="health-filters">
      <label className="flex flex-col gap-1 text-sm">
        {t("health.filterType")}
        <select
          className={select}
          value={props.type}
          onChange={(e) => props.setType(e.target.value)}
        >
          <option value="">{t("health.anyType")}</option>
          {(types.data ?? [])
            .filter((ty) => ty.name !== "media")
            .map((ty) => (
              <option key={ty.name} value={ty.name}>
                {ty.label}
              </option>
            ))}
        </select>
      </label>
      {locales.multilingual ? (
        <label className="flex flex-col gap-1 text-sm">
          {t("health.filterLocale")}
          <select
            className={select}
            value={props.locale}
            onChange={(e) => props.setLocale(e.target.value)}
          >
            <option value="">{t("health.anyLocale")}</option>
            {locales.all.map((l) => (
              <option key={l.tag} value={l.tag}>
                {l.label}
              </option>
            ))}
          </select>
        </label>
      ) : null}
      <label className="flex flex-col gap-1 text-sm">
        {t("health.filterSeverity")}
        <select
          className={select}
          value={props.severity}
          onChange={(e) => props.setSeverity(e.target.value)}
        >
          {SEVERITY_FILTERS.map((s) => (
            <option key={s} value={s}>
              {s ? t(`severity.${s}`) : t("health.anySeverity")}
            </option>
          ))}
        </select>
      </label>
    </div>
  );
}

function FindingsTable({ filters }: { filters: HealthFilters }) {
  const { t } = useTranslation("quality");
  const q = useInfiniteQuery(healthFindingsQuery(filters));
  if (q.isError) return <ErrorState error={q.error} retry={() => void q.refetch()} />;
  if (!q.data) return <Skeleton className="h-40 w-full" />;
  const rows = q.data.pages.flatMap((p) => p.items);
  if (rows.length === 0) {
    return <p className="text-sm text-muted-foreground">{t("health.noFindings")}</p>;
  }
  return (
    <div className="flex flex-col gap-3">
      <div className="overflow-x-auto rounded-xl border">
        <table className="w-full text-sm" data-slot="health-findings">
          <thead className="bg-muted/40 text-left">
            <tr>
              <th scope="col" className="px-3 py-2 font-medium">
                {t("health.colProblem")}
              </th>
              <th scope="col" className="px-3 py-2 font-medium">
                {t("health.colWhere")}
              </th>
              <th scope="col" className="px-3 py-2 font-medium">
                {t("health.colSeverity")}
              </th>
            </tr>
          </thead>
          <tbody>
            {rows.map((f, i) => {
              const href = findingHref(f);
              return (
                <tr key={`${f.rule}-${f.key}-${i}`} className="border-t" data-rule={f.rule}>
                  <td className="px-3 py-2">
                    <p className="font-medium">{titleFor(t, f.rule)}</p>
                    <p className="text-muted-foreground">{messageFor(t, f)}</p>
                    {f.suppressed ? (
                      <p className="text-xs text-muted-foreground">
                        {t("panel.suppressedBy", { reason: f.suppressed.reason })}
                      </p>
                    ) : null}
                  </td>
                  <td className="px-3 py-2">
                    {f.item && href ? (
                      <Link
                        to="/content/$type/$id"
                        params={{ type: f.item.type, id: f.item.id }}
                        search={href.search}
                        className="inline-flex min-h-11 items-center underline"
                      >
                        {f.item.title || t("health.untitled")}
                      </Link>
                    ) : f.redirect ? (
                      <Link
                        to="/structure/redirects"
                        className="inline-flex min-h-11 items-center font-mono underline"
                      >
                        {f.redirect.source}
                      </Link>
                    ) : null}
                    {f.item ? (
                      <p className="text-xs text-muted-foreground">
                        {f.item.type} · {f.item.locale}
                      </p>
                    ) : null}
                  </td>
                  <td className="px-3 py-2">{t(`severity.${f.severity}`)}</td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
      {q.hasNextPage ? (
        <Button
          type="button"
          variant="outline"
          className="self-start"
          disabled={q.isFetchingNextPage}
          onClick={() => void q.fetchNextPage()}
        >
          {t("health.more")}
        </Button>
      ) : null}
    </div>
  );
}
