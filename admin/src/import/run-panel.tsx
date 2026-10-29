// The run panel (P32): the job being followed, polled every 2 s while it is queued or
// running (`importJobQuery`) and left alone once it is done or dead. Its phase, the step
// stepper, when it started and its last heartbeat, the counts per kind, the error, and
// the report: the page in a new tab, the JSON as a download (a failed run's partial
// report too). Phase changes are announced once, by a live region that mounts empty.

import { useQuery } from "@tanstack/react-query";
import { cn } from "cn";
import {
  CheckIcon,
  CircleAlertIcon,
  CircleCheckIcon,
  HourglassIcon,
  LoaderCircleIcon,
  RotateCwIcon,
  SearchXIcon,
  XIcon,
} from "lucide-react";
import { useEffect, useMemo, useRef } from "react";
import { useTranslation } from "react-i18next";
import {
  type ImportCountsDto,
  type ImportJobDto,
  type ImportRunDto,
  importJobQuery,
} from "@/api/import";
import { ApiError } from "@/api/problem";
import { Alert } from "@/auth/auth-layout";
import { EmptyState } from "@/components/kit/empty-state";
import { ErrorState } from "@/components/kit/error-state";
import { RelativeTime } from "@/components/kit/relative-time";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { uiLocale } from "@/lib/locale";
import { ExpandableText, Fact } from "@/system/system-parts";
import {
  COUNT_COLUMNS,
  type ImportPhase,
  KNOWN_KINDS,
  type StepState,
  countTotals,
  importPhase,
  orderedKinds,
  stalled,
  stepStates,
} from "./import-model";
import { DryRunBadge, ReportActions } from "./parts";

export interface RunPanelProps {
  jobId: string;
  /** Once, when the job reaches `done` or `dead` (the screen refreshes the runs). */
  onSettled?: (job: ImportJobDto) => void;
  /** "New import": stop following this job. */
  onDismiss: () => void;
  /** Move focus to the panel's heading once it shows (the start card just went away). */
  focusHeading?: boolean;
}

const TERMINAL: ReadonlySet<ImportPhase> = new Set(["done", "failed"]);

export function RunPanel({ jobId, onSettled, onDismiss, focusHeading = false }: RunPanelProps) {
  const { t } = useTranslation("import");
  const job = useQuery(importJobQuery(jobId));
  const phase = job.data ? importPhase(job.data) : null;
  const step = job.data?.run?.step ?? null;
  const terminal = job.data ? job.data.status === "done" || job.data.status === "dead" : false;
  const settledFor = useRef<string | null>(null);
  useEffect(() => {
    if (!job.data || !terminal || settledFor.current === jobId) return;
    settledFor.current = jobId;
    onSettled?.(job.data);
  }, [job.data, terminal, jobId, onSettled]);

  // The live region renders empty; its text is written after it is in the page (and on
  // each phase or step change), so the first phase ("Import queued.") is read too.
  const live = useRef<HTMLSpanElement>(null);
  useEffect(() => {
    if (!phase || !live.current) return;
    live.current.textContent =
      phase === "running"
        ? t("run.announce.running", { step: step ? t(`run.step.${step as "parse"}`) : "" })
        : t(`run.announce.${phase}`);
  }, [phase, step, t]);

  const heading = useRef<HTMLHeadingElement>(null);
  const focused = useRef(false);
  const shown = phase !== null;
  useEffect(() => {
    if (!focusHeading || !shown || focused.current) return;
    focused.current = true;
    heading.current?.focus();
  }, [focusHeading, shown]);

  if (job.isError && !job.data) {
    if (job.error instanceof ApiError && job.error.status === 404) {
      return (
        <EmptyState
          icon={SearchXIcon}
          title={t("run.notFound")}
          body={t("description")}
          action={
            <Button variant="outline" onClick={onDismiss}>
              {t("run.another")}
            </Button>
          }
        />
      );
    }
    return <ErrorState error={job.error} retry={() => void job.refetch()} />;
  }
  if (!job.data || !phase) {
    return (
      <div aria-busy="true" data-slot="import-run-loading" className="rounded-xl border p-5">
        <Skeleton className="h-6 w-48" />
        <Skeleton className="mt-6 h-10 w-full" />
        <Skeleton className="mt-6 h-24 w-full" />
      </div>
    );
  }
  const data = job.data;
  const run = data.run ?? null;
  const error = run?.error ?? (phase === "failed" || phase === "retrying" ? data.last_error : null);

  return (
    <section
      data-slot="import-run"
      data-phase={phase}
      aria-label={t("run.label")}
      className="flex min-w-0 flex-col rounded-xl border bg-card shadow-1"
    >
      <span
        ref={live}
        data-slot="import-run-live"
        className="sr-only"
        aria-live="polite"
        aria-atomic="true"
      />
      <header className="flex min-w-0 flex-col gap-4 border-b px-5 py-4 md:flex-row md:items-start md:justify-between">
        <div className="flex min-w-0 items-start gap-3">
          <PhaseIcon phase={phase} />
          <div className="flex min-w-0 flex-col gap-1">
            <div className="flex flex-wrap items-center gap-2">
              <h2
                ref={heading}
                tabIndex={-1}
                className="rounded-sm text-md font-medium text-foreground focus-visible:ring-3 focus-visible:ring-ring/50 focus-visible:outline-none"
              >
                {t(`run.phase.${phase}`)}
              </h2>
              {run?.dry_run ? <DryRunBadge /> : null}
            </div>
            <p className="text-sm text-muted-foreground">
              {phase === "retrying"
                ? t("run.phaseBody.retrying", { attempts: data.attempts })
                : phase === "done" && run?.dry_run
                  ? t("run.phaseBody.doneDry")
                  : t(`run.phaseBody.${phase}`)}
            </p>
            {run?.source ? (
              <p className="min-w-0 truncate text-sm text-muted-foreground" title={run.source}>
                {t("run.source", { source: run.source })}
              </p>
            ) : null}
          </div>
        </div>
        {run ? (
          <dl className="grid shrink-0 grid-cols-2 gap-x-6 gap-y-2 sm:flex sm:gap-6">
            <Fact label={t("run.started")}>
              <RelativeTime at={run.started_at} />
            </Fact>
            {run.finished_at ? (
              <Fact label={t("run.finished")}>
                <RelativeTime at={run.finished_at} />
              </Fact>
            ) : (
              <Fact label={t("run.heartbeat")}>
                <RelativeTime at={run.heartbeat_at} />
              </Fact>
            )}
          </dl>
        ) : null}
      </header>

      <div className="flex min-w-0 flex-col gap-6 px-5 py-5">
        <Stepper run={run} phase={phase} />
        {stalled(run) ? <Alert tone="warning">{t("run.stalled")}</Alert> : null}
        {error ? (
          <div
            data-slot="import-run-error"
            className="flex min-w-0 flex-col gap-1.5 rounded-lg border border-destructive/40 bg-destructive/6 px-3 py-2.5"
          >
            <span className="flex items-center gap-2 text-sm font-medium text-foreground">
              <CircleAlertIcon aria-hidden="true" className="size-4 shrink-0 text-destructive" />
              {t("run.error")}
            </span>
            <ExpandableText text={error} className="text-foreground" />
          </div>
        ) : null}
        {run && Object.keys(run.counts).length > 0 ? <Counts counts={run.counts} /> : null}
      </div>

      {TERMINAL.has(phase) ? (
        <footer className="flex flex-col gap-3 border-t px-5 py-4 sm:flex-row sm:items-center sm:justify-end">
          {run && !run.report ? (
            <p className="text-sm text-muted-foreground sm:mr-auto">{t("run.noReport")}</p>
          ) : null}
          <Button variant="outline" className="pointer-coarse:h-11" onClick={onDismiss}>
            {t("run.another")}
          </Button>
          {run?.report ? <ReportActions run={run} partial={phase === "failed"} /> : null}
        </footer>
      ) : null}
    </section>
  );
}

const PHASE_ICON: Record<ImportPhase, { icon: typeof CheckIcon; className: string }> = {
  queued: { icon: HourglassIcon, className: "text-muted-foreground" },
  running: { icon: LoaderCircleIcon, className: "text-info" },
  retrying: { icon: RotateCwIcon, className: "text-warning" },
  done: { icon: CircleCheckIcon, className: "text-success" },
  failed: { icon: CircleAlertIcon, className: "text-destructive" },
};

function PhaseIcon({ phase }: { phase: ImportPhase }) {
  const { icon: Icon, className } = PHASE_ICON[phase];
  return (
    <span className="flex size-10 shrink-0 items-center justify-center rounded-lg border bg-background shadow-1">
      <Icon aria-hidden="true" className={cn("size-5", className)} />
    </span>
  );
}

const STEP_DOT: Record<StepState, string> = {
  done: "border-primary bg-primary text-primary-foreground",
  current: "border-primary bg-background text-primary ring-3 ring-primary/15",
  failed: "border-destructive bg-destructive/6 text-destructive ring-3 ring-destructive/15",
  pending: "border-border bg-background text-muted-foreground",
};

/**
 * The seven steps as an ordered list: across the panel from 768 px, down it below. A
 * step's state is in its text too (never colour alone); the current one is
 * `aria-current="step"`.
 */
function Stepper({ run, phase }: { run: ImportRunDto | null; phase: ImportPhase }) {
  const { t } = useTranslation("import");
  const steps = stepStates(run, phase);
  return (
    <ol
      data-slot="import-stepper"
      aria-label={t("run.steps")}
      className="grid min-w-0 gap-0 md:grid-cols-7 md:gap-2"
    >
      {steps.map(({ step, state }, i) => (
        <li
          key={step}
          data-step={step}
          data-state={state}
          aria-current={state === "current" ? "step" : undefined}
          className="relative flex min-w-0 items-start gap-3 pb-4 last:pb-0 md:flex-col md:items-start md:gap-2 md:pb-0"
        >
          {/* The connector to the next step: down on phones, across from 768 px. */}
          {i < steps.length - 1 ? (
            <span
              aria-hidden="true"
              className={cn(
                "absolute top-7 bottom-0 left-3.5 w-px md:top-3.5 md:right-[-0.5rem] md:bottom-auto md:left-9 md:h-px md:w-auto",
                state === "done" ? "bg-primary" : "bg-border",
              )}
            />
          ) : null}
          <span
            aria-hidden="true"
            className={cn(
              "relative z-10 flex size-7 shrink-0 items-center justify-center rounded-full border text-xs font-medium tabular-nums transition-colors duration-(--dur-base) ease-out",
              STEP_DOT[state],
            )}
          >
            {state === "done" ? (
              <CheckIcon className="size-3.5" />
            ) : state === "failed" ? (
              <XIcon className="size-3.5" />
            ) : (
              i + 1
            )}
          </span>
          <span className="flex min-w-0 flex-col pt-1 md:pt-0">
            <span
              className={cn(
                "text-sm break-words",
                state === "pending" ? "text-muted-foreground" : "font-medium text-foreground",
              )}
            >
              {t(`run.step.${step}`)}
            </span>
            <span
              className={cn(
                "text-xs",
                state === "failed" ? "text-destructive" : "text-muted-foreground",
              )}
            >
              {t(`run.stepState.${state}`)}
            </span>
          </span>
        </li>
      ))}
    </ol>
  );
}

/** The counts per kind and their totals; its own scroll frame on narrow screens. */
function Counts({ counts }: { counts: Record<string, ImportCountsDto> }) {
  const { t } = useTranslation("import");
  const number = useMemo(() => new Intl.NumberFormat(uiLocale()), []);
  const kinds = orderedKinds(counts);
  const totals = countTotals(counts);
  const known = new Set<string>(KNOWN_KINDS);
  const cell = "px-3 py-2 text-right tabular-nums whitespace-nowrap";
  const zero = (n: number) => (n === 0 ? "text-muted-foreground" : "text-foreground");
  return (
    <div data-slot="import-counts" className="flex min-w-0 flex-col gap-2">
      <h3 id="import-counts-title" className="text-sm font-medium text-foreground">
        {t("run.counts")}
      </h3>
      <div
        role="region"
        aria-labelledby="import-counts-title"
        tabIndex={0}
        className="min-w-0 overflow-x-auto rounded-lg border focus-visible:ring-3 focus-visible:ring-ring/50 focus-visible:outline-none"
      >
        <table className="w-full border-collapse text-sm">
          <thead className="bg-muted/50 text-xs text-muted-foreground">
            <tr>
              <th scope="col" className="px-3 py-2 text-left font-medium">
                {t("run.kind")}
              </th>
              {COUNT_COLUMNS.map((c) => (
                <th key={c} scope="col" className="px-3 py-2 text-right font-medium">
                  {t(`columns.${c}`)}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {kinds.map((kind) => {
              const row = counts[kind];
              if (!row) return null;
              return (
                <tr key={kind} data-kind={kind} className="border-t">
                  <th scope="row" className="px-3 py-2 text-left font-normal whitespace-nowrap">
                    {known.has(kind) ? (
                      t(`kinds.${kind as "users"}`)
                    ) : (
                      <code className="font-mono text-xs">{kind}</code>
                    )}
                  </th>
                  {COUNT_COLUMNS.map((c) => (
                    <td
                      key={c}
                      className={cn(
                        cell,
                        c === "failed" && row[c] > 0 ? "text-destructive" : zero(row[c]),
                      )}
                    >
                      {number.format(row[c])}
                    </td>
                  ))}
                </tr>
              );
            })}
          </tbody>
          <tfoot className="border-t bg-muted/30 font-medium">
            <tr>
              <th scope="row" className="px-3 py-2 text-left font-medium">
                {t("run.total")}
              </th>
              {COUNT_COLUMNS.map((c) => (
                <td key={c} className={cn(cell, zero(totals[c]))}>
                  {number.format(totals[c])}
                </td>
              ))}
            </tr>
          </tfoot>
        </table>
      </div>
    </div>
  );
}
