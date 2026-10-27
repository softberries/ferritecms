// Jobs (`GET /system/jobs`): every node's background jobs, newest first, 50 a page with
// Load more; the status chips and the kind (from the job kinds the nodes report) in the
// URL. A table of kind, status, attempts / max, run at, finished, the node holding it,
// and the last error (the server's scrubbed text, as text; a long one expands). Retry on
// a dead job (`POST /system/jobs/{id}/retry`); a kind no live node runs says so (H-Y1:
// the job waits for a node that knows the kind).

import {
  keepPreviousData,
  useInfiniteQuery,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { cn } from "cn";
import { ListChecksIcon, PlusIcon, RotateCwIcon, SearchXIcon, XIcon } from "lucide-react";
import { type ReactNode, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { reportApiError } from "@/api/client";
import { qk } from "@/api/keys";
import { nodesQuery } from "@/api/nodes";
import { ApiError } from "@/api/problem";
import type { SchemaJobDto } from "@/api/schema";
import { listJobs, retryJob } from "@/api/system";
import { DataTable } from "@/components/kit/data-table";
import { type DataTableColumn, dataTableColumns } from "@/components/kit/data-table-features";
import { EmptyState } from "@/components/kit/empty-state";
import { ErrorState } from "@/components/kit/error-state";
import { RelativeTime } from "@/components/kit/relative-time";
import { ScrollFade } from "@/components/kit/scroll-fade";
import { Button } from "@/components/ui/button";
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Chip } from "@/content/filters";
import {
  JOB_STATUSES,
  type JobStatus,
  SYSTEM_PAGE,
  type SystemSearch,
  type SystemSearchPatch,
} from "./system-model";
import { ExpandableText } from "./system-parts";

type JobDto = SchemaJobDto;

export interface TabProps {
  search: SystemSearch;
  setSearch: (patch: SystemSearchPatch) => void;
}

export function JobsTab({ search, setSearch }: TabProps) {
  const { t } = useTranslation("admin");
  const queryClient = useQueryClient();
  const filters = useMemo(
    () => ({
      ...(search.status ? { status: search.status } : {}),
      ...(search.kind ? { kind: search.kind } : {}),
    }),
    [search.status, search.kind],
  );
  const list = useInfiniteQuery({
    queryKey: qk.system.jobsInfinite(filters),
    queryFn: ({ pageParam, signal }) =>
      listJobs(
        { ...filters, limit: SYSTEM_PAGE, ...(pageParam ? { cursor: pageParam } : {}) },
        {
          signal,
        },
      ),
    initialPageParam: "",
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    placeholderData: keepPreviousData,
  });
  const nodes = useQuery(nodesQuery);
  const nodeNames = useMemo(
    () => new Map((nodes.data ?? []).map((n) => [n.id, n.name])),
    [nodes.data],
  );
  const kinds = useMemo(() => {
    const all = new Set<string>();
    for (const n of nodes.data ?? []) for (const k of n.meta?.job_kinds ?? []) all.add(k);
    if (search.kind) all.add(search.kind);
    return [...all].sort();
  }, [nodes.data, search.kind]);
  const jobs = useMemo(() => {
    const seen = new Map<string, JobDto>();
    for (const page of list.data?.pages ?? []) {
      for (const j of page.items) if (!seen.has(j.id)) seen.set(j.id, j);
    }
    return [...seen.values()];
  }, [list.data]);
  const [retrying, setRetrying] = useState<ReadonlySet<string>>(new Set());

  const retry = async (job: JobDto) => {
    setRetrying((s) => new Set(s).add(job.id));
    try {
      const done = await retryJob(job.id);
      if (done.handled === false) {
        toast.warning(t("system.jobs.retry.queuedUnhandled"), {
          description: t("system.jobs.retry.unhandledHint", { kind: done.kind }),
        });
      } else {
        toast.success(t("system.jobs.retry.queued"));
      }
    } catch (e) {
      reportApiError(e);
      const detail = e instanceof ApiError ? e.problem.detail || e.problem.title : undefined;
      toast.error(t("system.jobs.retry.failed"), detail ? { description: detail } : {});
    } finally {
      setRetrying((s) => {
        const next = new Set(s);
        next.delete(job.id);
        return next;
      });
      void queryClient.invalidateQueries({ queryKey: qk.system.jobLists });
      void queryClient.invalidateQueries({ queryKey: qk.system.status });
    }
  };

  const columns = useColumns(nodeNames);
  const filtered = Boolean(search.status || search.kind);
  let empty: ReactNode;
  if (search.kind) {
    empty = (
      <EmptyState
        icon={SearchXIcon}
        title={t("system.jobs.noMatch.title")}
        body={t("system.jobs.noMatch.body")}
        action={
          <Button
            variant="outline"
            onClick={() => setSearch({ status: undefined, kind: undefined })}
          >
            {t("system.jobs.clearFilters")}
          </Button>
        }
        className="border-0"
      />
    );
  } else if (search.status) {
    empty = (
      <EmptyState
        icon={ListChecksIcon}
        title={t(`system.jobs.empty.${search.status}`)}
        body={t("system.jobs.empty.statusBody")}
        className="border-0"
      />
    );
  } else {
    empty = (
      <EmptyState
        icon={ListChecksIcon}
        title={t("system.jobs.empty.title")}
        body={t("system.jobs.empty.body")}
        className="border-0"
      />
    );
  }

  return (
    <div data-slot="system-jobs" className="flex flex-col gap-4">
      <div role="search" className="min-w-0">
        <ScrollFade className="-m-1 min-w-0 p-1 lg:overflow-visible">
          <div className="flex w-max items-center gap-2 lg:w-auto lg:flex-wrap">
            <div role="group" aria-label={t("system.jobs.filterStatus")} className="flex gap-2">
              {JOB_STATUSES.map((s) => (
                <Chip
                  key={s}
                  pressed={search.status === s}
                  onClick={() => setSearch({ status: search.status === s ? undefined : s })}
                >
                  {t(`system.jobs.status.${s}`)}
                </Chip>
              ))}
            </div>
            <span aria-hidden="true" className="mx-1 h-5 w-px shrink-0 bg-border" />
            <KindPicker
              kinds={kinds}
              selected={search.kind}
              onSelect={(kind) => setSearch({ kind })}
            />
            {filtered ? (
              <Button
                variant="ghost"
                className="text-muted-foreground"
                onClick={() => setSearch({ status: undefined, kind: undefined })}
              >
                {t("system.jobs.clearFilters")}
              </Button>
            ) : null}
          </div>
        </ScrollFade>
      </div>
      {list.isError && !list.data ? (
        <ErrorState error={list.error} retry={() => void list.refetch()} />
      ) : (
        <DataTable
          label={t("system.tabs.jobs")}
          columns={columns}
          data={jobs}
          getRowId={(j) => j.id}
          loading={list.isPending}
          align="top"
          rowActions={(j) =>
            j.status === "dead" ? (
              <Button
                variant="outline"
                size="sm"
                className="pointer-coarse:h-11"
                aria-label={t("system.jobs.retry.on", { kind: j.kind })}
                disabled={retrying.has(j.id)}
                aria-busy={retrying.has(j.id) || undefined}
                onClick={() => void retry(j)}
              >
                <RotateCwIcon data-icon="inline-start" />
                {t("system.jobs.retry.action")}
              </Button>
            ) : null
          }
          columnClassName={{
            kind: "w-full max-w-0 py-2.5 md:w-[50%]",
            status: "hidden py-3 sm:table-cell",
            attempts: "hidden py-3 whitespace-nowrap md:table-cell",
            run_at: "hidden py-3 whitespace-nowrap xl:table-cell",
            finished: "hidden py-3 whitespace-nowrap lg:table-cell",
            locked_by: "hidden max-w-0 py-3 xl:table-cell xl:w-[12%]",
          }}
          className={
            list.isPlaceholderData ? "opacity-60 transition-opacity" : "transition-opacity"
          }
          empty={empty}
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
                  {list.isFetchingNextPage
                    ? t("system.jobs.loadingMore")
                    : t("system.jobs.loadMore")}
                </Button>
              </div>
            ) : null
          }
        />
      )}
    </div>
  );
}

const STATUS_TONE: Record<JobStatus, string> = {
  queued: "text-muted-foreground",
  running: "text-info",
  done: "text-success",
  dead: "text-destructive",
};

/** A job status as a dot plus a label, never colour alone. */
export function JobStatusBadge({ status }: { status: string }) {
  const { t } = useTranslation("admin");
  const known = (JOB_STATUSES as readonly string[]).includes(status);
  const s = status as JobStatus;
  return (
    <span
      data-slot="job-status"
      data-status={status}
      className={cn(
        "inline-flex h-6 shrink-0 items-center gap-1.5 rounded-full border px-2 text-xs font-medium whitespace-nowrap",
        known ? STATUS_TONE[s] : "text-muted-foreground",
      )}
    >
      <span
        aria-hidden="true"
        className={cn(
          "size-2 shrink-0 rounded-full border-[1.5px] border-current",
          status === "queued" ? "bg-transparent" : "bg-current",
        )}
      />
      {known ? t(`system.jobs.status.${s}`) : status}
    </span>
  );
}

function Attempts({ job }: { job: JobDto }) {
  const { t } = useTranslation("admin");
  return (
    <span
      className="text-sm text-muted-foreground tabular-nums"
      title={t("system.jobs.attemptsLabel", { attempts: job.attempts, max: job.max_attempts })}
    >
      {t("system.jobs.attempts", { attempts: job.attempts, max: job.max_attempts })}
    </span>
  );
}

function useColumns(nodeNames: ReadonlyMap<string, string>): DataTableColumn<JobDto>[] {
  const { t } = useTranslation("admin");
  return useMemo(() => {
    const col = dataTableColumns<JobDto>();
    const none = <span className="text-muted-foreground">{t("system.jobs.none")}</span>;
    return [
      col.accessor("kind", {
        id: "kind",
        header: () => t("system.jobs.columns.job"),
        cell: (c) => {
          const j = c.row.original;
          return (
            <div className="flex min-w-0 flex-col gap-1">
              <code className="truncate font-mono text-sm text-foreground" title={j.kind}>
                {j.kind}
              </code>
              {j.handled === false ? (
                <span data-slot="job-unhandled" className="text-xs font-medium text-warning">
                  {t("system.jobs.unhandled")}
                </span>
              ) : null}
              {/* Below `md` the status and attempts columns are hidden: here. */}
              <span className="flex flex-wrap items-center gap-2 md:hidden">
                <span className="sm:hidden">
                  <JobStatusBadge status={j.status} />
                </span>
                <Attempts job={j} />
                {j.finished_at ? (
                  <RelativeTime at={j.finished_at} className="text-xs text-muted-foreground" />
                ) : null}
              </span>
              {j.last_error ? <ExpandableText text={j.last_error} /> : null}
            </div>
          );
        },
      }),
      col.accessor("status", {
        id: "status",
        header: () => t("system.jobs.columns.status"),
        cell: (c) => <JobStatusBadge status={c.row.original.status} />,
      }),
      col.accessor("attempts", {
        id: "attempts",
        header: () => t("system.jobs.columns.attempts"),
        cell: (c) => <Attempts job={c.row.original} />,
      }),
      col.accessor("run_at", {
        id: "run_at",
        header: () => t("system.jobs.columns.runAt"),
        cell: (c) => (
          <RelativeTime at={c.row.original.run_at} className="text-sm text-muted-foreground" />
        ),
      }),
      col.accessor("finished_at", {
        id: "finished",
        header: () => t("system.jobs.columns.finished"),
        cell: (c) =>
          c.row.original.finished_at ? (
            <RelativeTime
              at={c.row.original.finished_at}
              className="text-sm text-muted-foreground"
            />
          ) : (
            none
          ),
      }),
      col.accessor("locked_by", {
        id: "locked_by",
        header: () => t("system.jobs.columns.lockedBy"),
        cell: (c) => {
          const id = c.row.original.locked_by;
          if (!id) return none;
          const name = nodeNames.get(id) ?? id;
          return (
            <span className="block truncate font-mono text-xs" title={id}>
              {name}
            </span>
          );
        },
      }),
    ];
  }, [t, nodeNames]);
}

function KindPicker({
  kinds,
  selected,
  onSelect,
}: {
  kinds: string[];
  selected: string | undefined;
  onSelect: (kind: string | undefined) => void;
}) {
  const { t } = useTranslation("admin");
  const [open, setOpen] = useState(false);
  return (
    <div className="flex items-center">
      <Popover open={open} onOpenChange={setOpen}>
        <PopoverTrigger asChild>
          <Chip
            pressed={selected !== undefined}
            dashed
            aria-label={
              selected
                ? t("system.jobs.kind.chosen", { kind: selected })
                : t("system.jobs.kind.label")
            }
            className={selected ? "rounded-r-none border-r-0 pr-2" : undefined}
          >
            {selected ? (
              <>
                <span className="text-muted-foreground">{t("system.jobs.kind.label")}</span>
                <span className="max-w-48 truncate font-mono" title={selected}>
                  {selected}
                </span>
              </>
            ) : (
              <>
                <PlusIcon data-icon="inline-start" />
                {t("system.jobs.kind.label")}
              </>
            )}
          </Chip>
        </PopoverTrigger>
        <PopoverContent align="start" className="w-64 p-0">
          <Command
            label={t("system.jobs.kind.search")}
            loop
            className="**:data-[slot=input-group]:outline-none!"
          >
            <CommandInput placeholder={t("system.jobs.kind.search")} />
            <CommandList>
              <CommandEmpty>{t("system.jobs.kind.none")}</CommandEmpty>
              <CommandGroup>
                {kinds.map((k) => (
                  <CommandItem
                    key={k}
                    value={k}
                    data-checked={k === selected}
                    onSelect={() => {
                      onSelect(k === selected ? undefined : k);
                      setOpen(false);
                    }}
                  >
                    <span className="truncate font-mono" title={k}>
                      {k}
                    </span>
                  </CommandItem>
                ))}
              </CommandGroup>
            </CommandList>
          </Command>
        </PopoverContent>
      </Popover>
      {selected ? (
        <Chip
          pressed
          aria-label={t("system.jobs.kind.clear")}
          onClick={() => onSelect(undefined)}
          className="rounded-l-none border-l-primary/20 px-2"
        >
          <XIcon />
        </Chip>
      ) : null}
    </div>
  );
}
