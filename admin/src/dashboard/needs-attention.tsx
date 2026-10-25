// Needs attention: what waits for the actor, as a list of checks. Pending review for
// roles with `edit_others`; for `manage_system` dead jobs, the schema verdict and theme
// fallback nodes (`GET /system/status`); for `manage_plugins` plugins disabled by
// failures. A check the actor may not see is not requested at all, and each check
// loads and fails on its own. System entries are plain text until M2b-3 adds their
// detail screens.

import { type UseQueryResult, useQueries, useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { CircleAlertIcon, CircleCheckIcon, RotateCwIcon, TriangleAlertIcon } from "lucide-react";
import { cn } from "cn";
import { Children, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { listContent } from "@/api/content";
import { qk } from "@/api/keys";
import { pluginsQuery } from "@/api/plugins";
import { jobsQuery, systemStatusQuery } from "@/api/system";
import { RelativeTime } from "@/components/kit/relative-time";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { uiLocale } from "@/lib/locale";
import { Panel } from "./panel";
import { type AttentionAccess, CARD_LIMIT, PENDING_FILTERS, newest } from "./queries";

const DEAD = { status: "dead", limit: CARD_LIMIT };
const TYPES = ["post", "page"] as const;

type Tone = "ok" | "warn" | "bad";

export function NeedsAttention({
  access,
  className,
}: {
  access: AttentionAccess;
  className?: string;
}) {
  const { t } = useTranslation("dashboard");
  const pending = useQueries({
    queries: TYPES.map((type) => ({
      queryKey: qk.content.list(type, PENDING_FILTERS),
      queryFn: ({ signal }: { signal: AbortSignal }) =>
        listContent(type, PENDING_FILTERS, { signal }),
      staleTime: 30_000,
      enabled: access.pending,
    })),
  });
  const jobs = useQuery({ ...jobsQuery(DEAD), enabled: access.system });
  const status = useQuery({ ...systemStatusQuery, enabled: access.system });
  const plugins = useQuery({ ...pluginsQuery, enabled: access.plugins });

  const pendingItems = pending.every((q) => q.data)
    ? newest(
        pending.map((q) => q.data?.items ?? []),
        CARD_LIMIT,
      )
    : null;
  const disabled = plugins.data?.filter((p) => !p.enabled && p.disabled_reason) ?? null;
  const verdict = status.data?.schema.verdict;
  const fallback = status.data?.theme.fallback_nodes ?? null;

  // The true number of dead jobs: the status counts, else the (capped) page.
  const deadCount = status.data?.jobs.dead ?? jobs.data?.items.length ?? 0;

  // How many checks need attention (not entries), for the header line.
  const open = [
    (pendingItems?.length ?? 0) > 0,
    (jobs.data?.items.length ?? 0) > 0,
    (disabled?.length ?? 0) > 0,
    verdict !== undefined && verdict !== "up_to_date",
    (fallback?.length ?? 0) > 0,
  ].filter(Boolean).length;
  const loading =
    (access.pending && pending.some((q) => q.isPending)) ||
    (access.system && (jobs.isPending || status.isPending)) ||
    (access.plugins && plugins.isPending);

  return (
    <Panel
      title={t("attention.title")}
      description={
        loading
          ? t("attention.checking")
          : open === 0
            ? t("attention.allClear")
            : t("attention.open", { count: open })
      }
      busy={loading}
      className={className}
    >
      <ul className="-mx-2 flex flex-col divide-y">
        {access.pending ? (
          <Check
            label={t("attention.pending.label")}
            query={combine(pending)}
            tone={pendingItems && pendingItems.length > 0 ? "warn" : "ok"}
            summary={
              pendingItems && pendingItems.length > 0
                ? t("attention.pending.some", { count: pendingItems.length })
                : t("attention.pending.none")
            }
            action={
              pendingItems && pendingItems.length > 0 ? (
                <Link
                  to="/content/$type"
                  params={{ type: "post" }}
                  search={{ status: "pending" }}
                  className="text-sm font-medium text-primary hover:underline"
                >
                  {t("attention.pending.all")}
                </Link>
              ) : null
            }
          >
            {pendingItems?.map((item) => (
              <li key={item.id}>
                <Link
                  to={`/content/${item.type}/${item.id}` as "/"}
                  className="flex min-h-11 items-center gap-3 rounded-md px-2 py-1.5 hover:bg-muted/60"
                >
                  <span className="min-w-0 flex-1 truncate" title={item.title}>
                    {item.title || t("drafts.untitled")}
                  </span>
                  <span className="shrink-0 text-xs text-muted-foreground">
                    {item.author.display_name || item.author.login} ·{" "}
                    <RelativeTime at={item.updated_at} />
                  </span>
                </Link>
              </li>
            ))}
          </Check>
        ) : null}

        {access.system ? (
          <Check
            label={t("attention.jobs.label")}
            query={jobs}
            tone={jobs.data && jobs.data.items.length > 0 ? "bad" : "ok"}
            summary={
              jobs.data && jobs.data.items.length > 0
                ? t("attention.jobs.some", {
                    count: Math.max(deadCount, jobs.data.items.length),
                  })
                : t("attention.jobs.none")
            }
          >
            {jobs.data?.items.map((job) => (
              <li
                key={job.id}
                className="flex min-h-11 flex-col justify-center gap-0.5 px-2 py-1.5"
              >
                <span className="flex flex-wrap items-center gap-x-2 gap-y-0.5">
                  <code className="font-mono text-xs text-foreground">{job.kind}</code>
                  <span className="text-xs whitespace-nowrap text-muted-foreground tabular-nums">
                    {t("attention.jobs.attempts", {
                      count: job.attempts,
                      attempts: job.attempts,
                      max: job.max_attempts,
                    })}
                  </span>
                  {job.finished_at ? (
                    <RelativeTime
                      at={job.finished_at}
                      className="ml-auto text-xs whitespace-nowrap text-muted-foreground"
                    />
                  ) : null}
                </span>
                {job.last_error ? (
                  <span className="truncate text-xs text-muted-foreground" title={job.last_error}>
                    {job.last_error}
                  </span>
                ) : null}
              </li>
            ))}
          </Check>
        ) : null}

        {access.plugins ? (
          <Check
            label={t("attention.plugins.label")}
            query={plugins}
            tone={disabled && disabled.length > 0 ? "bad" : "ok"}
            summary={
              disabled && disabled.length > 0
                ? t("attention.plugins.some", { count: disabled.length })
                : t("attention.plugins.none")
            }
          >
            {disabled?.map((p) => (
              <li
                key={p.name}
                className="flex min-h-11 flex-col justify-center gap-0.5 px-2 py-1.5"
              >
                <span className="flex items-center gap-2">
                  <code className="font-mono text-xs text-foreground">{p.name}</code>
                  <span className="text-xs text-muted-foreground">{p.version}</span>
                </span>
                <span
                  className="truncate text-xs text-muted-foreground"
                  title={p.disabled_reason ?? ""}
                >
                  {p.disabled_reason}
                </span>
              </li>
            ))}
          </Check>
        ) : null}

        {access.system ? (
          <>
            <Check
              label={t("attention.schema.label")}
              query={status}
              tone={
                verdict === "incompatible"
                  ? "bad"
                  : verdict && verdict !== "up_to_date"
                    ? "warn"
                    : "ok"
              }
              summary={
                status.data
                  ? t(`attention.schema.${status.data.schema.verdict}`, {
                      count: status.data.schema.pending.length,
                      db: status.data.schema.db_version,
                      binary: status.data.schema.binary_version,
                    })
                  : ""
              }
            />
            <Check
              label={t("attention.theme.label")}
              query={status}
              tone={fallback && fallback.length > 0 ? "warn" : "ok"}
              summary={
                fallback && fallback.length > 0
                  ? t("attention.theme.some", {
                      count: fallback.length,
                      formatted: new Intl.NumberFormat(uiLocale()).format(fallback.length),
                    })
                  : t("attention.theme.none")
              }
            >
              {fallback?.map((node) => (
                <li key={node} className="px-2 py-1">
                  <code className="font-mono text-xs break-all text-muted-foreground">{node}</code>
                </li>
              ))}
            </Check>
          </>
        ) : null}
      </ul>
    </Panel>
  );
}

/** Several queries as one check: pending while any is, failed while any has. */
function combine(queries: UseQueryResult[]): CheckQuery {
  const failed = queries.find((q) => q.isError);
  return {
    isPending: queries.some((q) => q.isPending),
    isError: failed !== undefined,
    refetch: () => {
      for (const q of queries) if (q.isError) void q.refetch();
    },
  };
}

interface CheckQuery {
  isPending: boolean;
  isError: boolean;
  refetch: () => unknown;
}

const TONE: Record<Tone, { icon: typeof CircleCheckIcon; className: string }> = {
  ok: { icon: CircleCheckIcon, className: "text-success" },
  warn: { icon: TriangleAlertIcon, className: "text-warning" },
  bad: { icon: CircleAlertIcon, className: "text-destructive" },
};

/** One check: an icon and a sentence (never colour alone), then its entries. */
function Check({
  label,
  query,
  tone,
  summary,
  action,
  children,
}: {
  label: string;
  query: CheckQuery;
  tone: Tone;
  summary: string;
  action?: ReactNode;
  children?: ReactNode;
}) {
  const { t } = useTranslation("dashboard");
  const { icon: Icon, className } = TONE[tone];
  return (
    <li className="flex flex-col gap-1 px-2 py-3" data-slot="attention-check">
      <div className="flex min-h-6 items-center gap-3">
        {query.isPending ? (
          <>
            <Skeleton className="size-4 rounded-full" />
            <span className="text-sm text-muted-foreground">{label}</span>
            <Skeleton className="ml-auto h-4 w-20" />
          </>
        ) : query.isError ? (
          <>
            <CircleAlertIcon aria-hidden="true" className="size-4 shrink-0 text-destructive" />
            <span className="min-w-0 flex-1 text-sm text-foreground">
              {t("attention.failed", { label })}
            </span>
            <Button
              variant="ghost"
              size="sm"
              onClick={() => {
                void query.refetch();
              }}
            >
              <RotateCwIcon data-icon="inline-start" />
              {t("attention.retry")}
            </Button>
          </>
        ) : (
          <>
            <Icon aria-hidden="true" className={cn("size-4 shrink-0", className)} />
            <span className="flex min-w-0 flex-1 flex-col">
              <span className="text-xs font-medium tracking-wide text-muted-foreground uppercase">
                {label}
              </span>
              <span className="text-sm text-foreground">{summary}</span>
            </span>
            {action}
          </>
        )}
      </div>
      {!query.isPending && !query.isError && Children.count(children) > 0 ? (
        <ul className="ml-5 flex flex-col">{children}</ul>
      ) : null}
    </li>
  );
}
