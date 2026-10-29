// The Import screen (07 §3 Import, P32; `import`): the page header, then either the start
// card or, while a job is followed, the run panel, then the runs table. The job followed
// is the URL's `?job=`. When the page opens and the newest run is still running (an
// import started elsewhere, or before a reload), its job is followed and pinned in the
// URL, so the finished panel and its report stay on show; a run that appears later (a
// busy start) is offered, never followed on its own. A finished job stays until "New
// import". Focus moves to the panel's heading after a start and to the start card's
// heading after "New import".

import { useInfiniteQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { IMPORT_WORDPRESS_KIND, type ImportJobDto, importRunsQuery } from "@/api/import";
import { qk } from "@/api/keys";
import { ApiError } from "@/api/problem";
import { ForbiddenState } from "@/components/kit/forbidden-state";
import { PageHeader } from "@/components/kit/page-header";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { RunPanel } from "./run-panel";
import { RunsTable } from "./runs-table";
import { StartCard } from "./start-card";

/** The query roots a finished import leaves alone (`qk.import`, `qk.me`, `qk.meta`). */
const KEPT_AFTER_IMPORT: ReadonlySet<string> = new Set([qk.import.runs[0], qk.me[0], qk.meta[0]]);

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
  const newest = runs.data?.pages[0]?.items[0];
  const runningJob = newest?.state === "running" ? (newest.job_id ?? undefined) : undefined;
  // Decided once, when the runs first arrive: the job of a run already running then.
  // `null` until decided.
  const [opened, setOpened] = useState<string | undefined | null>(null);
  if (opened === null && runs.data) setOpened(runningJob);
  const followed = job ?? opened ?? undefined;
  useEffect(() => {
    if (!job && opened) setJob(opened);
  }, [job, opened, setJob]);
  // Where focus goes when the start card and the panel swap.
  const [focus, setFocus] = useState<"panel" | "start" | null>(null);
  const refreshRuns = useCallback(() => {
    void queryClient.invalidateQueries({ queryKey: qk.import.runs });
  }, [queryClient]);
  const onSettled = useCallback(
    (settled: ImportJobDto) => {
      refreshRuns();
      // A real import that finished wrote types, terms, content, media, menus, users and
      // settings: every screen's data is stale but the session's (`me`) and the site's
      // facts (`meta`), and the import's own (just refreshed).
      if (settled.status === "done" && settled.run?.state === "done" && !settled.run.dry_run) {
        void queryClient.invalidateQueries({
          predicate: (q) => !KEPT_AFTER_IMPORT.has(String(q.queryKey[0])),
        });
      }
    },
    [queryClient, refreshRuns],
  );

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
          focusHeading={focus === "panel"}
          onSettled={onSettled}
          onDismiss={() => {
            setOpened(undefined);
            setFocus("start");
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
          focusHeading={focus === "start"}
          onStarted={(started) => {
            queryClient.setQueryData(qk.import.job(started.job_id), started);
            setFocus("panel");
            setJob(started.job_id);
            refreshRuns();
            // The System screen's job list and counts gained a queued job.
            void queryClient.invalidateQueries({ queryKey: qk.system.jobLists });
            void queryClient.invalidateQueries({ queryKey: qk.system.status });
          }}
          onBusy={refreshRuns}
          busyAction={
            runningJob ? (
              <Button
                variant="outline"
                className="pointer-coarse:h-11"
                onClick={() => {
                  setFocus("panel");
                  setJob(runningJob);
                }}
              >
                {t("start.busyShow")}
              </Button>
            ) : (
              // A queued job has no run yet: the System screen's job list shows it.
              <Button asChild variant="outline" className="pointer-coarse:h-11">
                <Link
                  to="/system/$tab"
                  params={{ tab: "jobs" }}
                  search={{ status: "queued", kind: IMPORT_WORDPRESS_KIND }}
                >
                  {t("start.busyJobs")}
                </Link>
              </Button>
            )
          }
        />
      )}
      <RunsTable />
    </div>
  );
}
