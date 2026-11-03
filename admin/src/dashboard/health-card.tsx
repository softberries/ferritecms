// The dashboard's "Content health" card (M6-4, design 16 §11.2): the newest run's
// errors, warnings and notices and when it ran, for `edit_others`.

import { Link } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { healthSummaryQuery } from "@/api/quality";
import { ErrorState } from "@/components/kit/error-state";
import { RelativeTime } from "@/components/kit/relative-time";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { severityTotals } from "@/health/health-model";
import { Panel } from "./panel";

export function HealthCard({ className }: { className?: string }) {
  const { t } = useTranslation("quality");
  const summary = useQuery(healthSummaryQuery);
  const totals = severityTotals(summary.data);
  return (
    <Panel
      title={t("health.card.title")}
      description={t("health.card.description")}
      busy={summary.isPending}
      className={className}
      action={
        <Button variant="ghost" size="sm" asChild className="min-h-11">
          <Link to="/health">{t("health.card.open")}</Link>
        </Button>
      }
    >
      <div data-slot="dashboard-health">
        {summary.isError ? (
          <ErrorState error={summary.error} retry={() => void summary.refetch()} />
        ) : !summary.data ? (
          <Skeleton className="h-16 w-full" />
        ) : !summary.data.last ? (
          <p className="text-sm text-muted-foreground">{t("health.card.never")}</p>
        ) : (
          <div className="flex flex-col gap-2">
            <dl className="grid grid-cols-3 gap-3">
              {(["error", "warning", "notice"] as const).map((s) => (
                <div key={s} className="flex flex-col">
                  <dt className="text-sm text-muted-foreground">{t(`severity.${s}`)}</dt>
                  <dd className="text-xl font-semibold tabular-nums">{totals[s]}</dd>
                </div>
              ))}
            </dl>
            {summary.data.last.finished_at ? (
              <p className="text-xs text-muted-foreground">
                {t("health.lastRun")} <RelativeTime at={summary.data.last.finished_at} />
              </p>
            ) : null}
          </div>
        )}
      </div>
    </Panel>
  );
}
