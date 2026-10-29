// The Import screen (07 §3 Import, P32; `import`): the page header, then either the start
// card or, while a job is followed, the run panel, then the runs table. The job followed
// is the URL's `?job=`, or else the job of the newest run if it still runs (an import
// started elsewhere, or before a reload), so the page never offers a second start beside
// one in progress. A finished job stays on show until "New import".

import { useInfiniteQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback, useState } from "react";
import { useTranslation } from "react-i18next";
import { importRunsQuery } from "@/api/import";
import { qk } from "@/api/keys";
import { ApiError } from "@/api/problem";
import { ForbiddenState } from "@/components/kit/forbidden-state";
import { PageHeader } from "@/components/kit/page-header";
import { Skeleton } from "@/components/ui/skeleton";
import { RunPanel } from "./run-panel";
import { RunsTable } from "./runs-table";
import { StartCard } from "./start-card";

export interface ImportScreenProps {
  /** The job the URL names (`?job=`). */
  job: string | undefined;
  setJob: (job: string | undefined) => void;
}

export function ImportScreen({ job, setJob }: ImportScreenProps) {
  const { t } = useTranslation("import");
  const queryClient = useQueryClient();
  // The runs table's query (one request): is the newest run still running?
  const runs = useInfiniteQuery(importRunsQuery);
  // Jobs "New import" put away: never followed again from the runs list.
  const [dismissed, setDismissed] = useState<ReadonlySet<string>>(new Set());
  const newest = runs.data?.pages[0]?.items[0];
  const running =
    newest?.state === "running" && newest.job_id && !dismissed.has(newest.job_id)
      ? newest.job_id
      : undefined;
  const followed = job ?? running;
  const refreshRuns = useCallback(() => {
    void queryClient.invalidateQueries({ queryKey: qk.import.runs });
  }, [queryClient]);

  const header = <PageHeader title={t("title")} description={t("description")} />;
  if (runs.error instanceof ApiError && runs.error.status === 403) {
    return (
      <div data-slot="import-screen" className="mx-auto flex w-full max-w-6xl flex-col gap-6">
        {header}
        <ForbiddenState />
      </div>
    );
  }

  return (
    <div data-slot="import-screen" className="mx-auto flex w-full max-w-6xl flex-col gap-6">
      {header}
      {followed ? (
        <RunPanel
          key={followed}
          jobId={followed}
          onSettled={refreshRuns}
          onDismiss={() => {
            setDismissed((d) => new Set(d).add(followed));
            refreshRuns();
            setJob(undefined);
          }}
        />
      ) : runs.isPending ? (
        <div aria-busy="true" data-slot="import-start-loading" className="rounded-xl border p-5">
          <Skeleton className="h-5 w-32" />
          <div className="mt-5 grid gap-4 lg:grid-cols-2">
            <Skeleton className="h-28 w-full rounded-lg" />
            <Skeleton className="h-28 w-full rounded-lg" />
          </div>
          <Skeleton className="mt-6 h-20 w-full" />
        </div>
      ) : (
        <StartCard
          onStarted={(started) => {
            queryClient.setQueryData(qk.import.job(started.job_id), started);
            setJob(started.job_id);
            refreshRuns();
          }}
          onBusy={refreshRuns}
        />
      )}
      <RunsTable />
    </div>
  );
}
