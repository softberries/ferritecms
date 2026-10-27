// Status (`GET /system/status`): what this site runs on (version, mode, database, media
// storage, address), the schema verdict in plain words, mail, the active theme and the
// nodes serving a fallback, jobs by status (each a link to the Jobs tab filtered) and the
// live and stale nodes (a link to the Nodes tab).

import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { cn } from "cn";
import { CircleCheckIcon, CircleDashedIcon } from "lucide-react";
import { useTranslation } from "react-i18next";
import { INLINE_ERRORS } from "@/api/client";
import { type SystemStatusDto, systemStatusQuery } from "@/api/system";
import { ErrorState } from "@/components/kit/error-state";
import { Skeleton } from "@/components/ui/skeleton";
import { ListSkeleton, Panel } from "@/dashboard/panel";
import { uiLocale } from "@/lib/locale";
import { JOB_STATUSES, schemaVerdict } from "./system-model";
import { Fact, ToneIcon } from "./system-parts";

export function StatusTab() {
  const status = useQuery({ ...systemStatusQuery, meta: INLINE_ERRORS });
  if (status.isError) {
    return <ErrorState error={status.error} retry={() => void status.refetch()} />;
  }
  if (!status.data) return <StatusSkeleton />;
  const s = status.data;
  return (
    <div data-slot="system-status" className="grid grid-cols-[minmax(0,1fr)] gap-4 md:grid-cols-2">
      <SiteFacts s={s} />
      <SchemaPanel s={s} />
      <JobsPanel s={s} />
      <NodesPanel s={s} />
      <ThemePanel s={s} />
      <MailPanel s={s} />
    </div>
  );
}

/** A value the API sends as a fixed word, named; an unknown one as it is. */
function useWord() {
  const { t } = useTranslation("admin");
  return {
    mode: (v: string) => (v === "solo" || v === "cluster" ? t(`system.status.modes.${v}`) : v),
    database: (v: string) =>
      v === "sqlite" || v === "postgres" ? t(`system.status.databases.${v}`) : v,
    media: (v: string) => (v === "local" || v === "s3" ? t(`system.status.medias.${v}`) : v),
  };
}

function SiteFacts({ s }: { s: SystemStatusDto }) {
  const { t } = useTranslation("admin");
  const word = useWord();
  return (
    <Panel title={t("system.status.site.title")}>
      <dl className="grid grid-cols-[minmax(0,1fr)] gap-4 sm:grid-cols-2">
        <Fact label={t("system.status.site.version")}>
          <span className="font-mono tabular-nums">{s.version}</span>
        </Fact>
        <Fact label={t("system.status.site.mode")}>{word.mode(s.mode)}</Fact>
        <Fact label={t("system.status.site.database")}>{word.database(s.database)}</Fact>
        <Fact label={t("system.status.site.media")}>{word.media(s.media)}</Fact>
        <Fact label={t("system.status.site.siteUrl")} className="sm:col-span-2">
          <span className="font-mono">{s.site_url}</span>
        </Fact>
      </dl>
    </Panel>
  );
}

function SchemaPanel({ s }: { s: SystemStatusDto }) {
  const { t } = useTranslation("admin");
  const verdict = schemaVerdict(t, s.schema);
  return (
    <Panel title={t("system.status.schema.title")}>
      <div className="flex flex-col gap-4">
        <div data-slot="schema-verdict" data-verdict={s.schema.verdict} className="flex gap-3">
          <ToneIcon tone={verdict.tone} className="mt-0.5" />
          <div className="flex min-w-0 flex-col gap-0.5">
            <p className="text-sm font-medium text-foreground">{verdict.title}</p>
            <p className="text-sm text-muted-foreground">{verdict.body}</p>
          </div>
        </div>
        <dl className="grid grid-cols-2 gap-4">
          <Fact label={t("system.status.schema.database")}>
            <span className="tabular-nums">{s.schema.db_version}</span>
          </Fact>
          <Fact label={t("system.status.schema.binary")}>
            <span className="tabular-nums">{s.schema.binary_version}</span>
          </Fact>
          {s.schema.pending.length > 0 ? (
            <Fact label={t("system.status.schema.pending")} className="col-span-2">
              <ul className="flex flex-wrap gap-1.5">
                {s.schema.pending.map((v) => (
                  <li
                    key={v}
                    className="rounded-md bg-muted px-1.5 font-mono text-xs leading-6 tabular-nums"
                  >
                    {v}
                  </li>
                ))}
              </ul>
            </Fact>
          ) : null}
        </dl>
      </div>
    </Panel>
  );
}

function JobsPanel({ s }: { s: SystemStatusDto }) {
  const { t } = useTranslation("admin");
  const number = new Intl.NumberFormat(uiLocale());
  return (
    <Panel title={t("system.status.jobs.title")} description={t("system.status.jobs.hint")}>
      <ul className="grid grid-cols-2 gap-2 sm:grid-cols-4">
        {JOB_STATUSES.map((status) => {
          const count = s.jobs[status];
          const alarm = status === "dead" && count > 0;
          return (
            <li key={status} className="min-w-0">
              <Link
                to="/system/$tab"
                params={{ tab: "jobs" }}
                search={{ status }}
                data-status={status}
                className={cn(
                  "flex min-h-16 flex-col justify-center gap-0.5 rounded-lg border px-3 py-2 transition-colors hover:bg-muted/60",
                  alarm && "border-destructive/40 bg-destructive/5",
                )}
              >
                <span className="text-xs text-muted-foreground">
                  {t(`system.jobs.status.${status}`)}
                </span>
                <span
                  className={cn(
                    "text-lg font-semibold tabular-nums",
                    alarm ? "text-destructive" : "text-foreground",
                  )}
                >
                  {number.format(count)}
                </span>
              </Link>
            </li>
          );
        })}
      </ul>
    </Panel>
  );
}

function NodesPanel({ s }: { s: SystemStatusDto }) {
  const { t } = useTranslation("admin");
  const number = new Intl.NumberFormat(uiLocale());
  const items = [
    { key: "live", count: s.nodes.live, alarm: s.nodes.live === 0 },
    { key: "stale", count: s.nodes.stale, alarm: false },
  ] as const;
  return (
    <Panel title={t("system.status.nodes.title")} description={t("system.status.nodes.hint")}>
      <ul className="grid grid-cols-2 gap-2">
        {items.map((item) => (
          <li key={item.key} className="min-w-0">
            <Link
              to="/system/$tab"
              params={{ tab: "nodes" }}
              className="flex min-h-16 flex-col justify-center gap-0.5 rounded-lg border px-3 py-2 transition-colors hover:bg-muted/60"
            >
              <span className="text-xs text-muted-foreground">
                {t(`system.status.nodes.${item.key}`)}
              </span>
              <span
                className={cn(
                  "text-lg font-semibold tabular-nums",
                  item.alarm ? "text-destructive" : "text-foreground",
                )}
              >
                {number.format(item.count)}
              </span>
            </Link>
          </li>
        ))}
      </ul>
    </Panel>
  );
}

function ThemePanel({ s }: { s: SystemStatusDto }) {
  const { t } = useTranslation("admin");
  const fallback = s.theme.fallback_nodes;
  return (
    <Panel title={t("system.status.theme.title")}>
      <div className="flex flex-col gap-3">
        <dl>
          <Fact label={t("system.status.theme.active")}>
            {s.theme.active ? (
              <span className="font-medium">{s.theme.active}</span>
            ) : (
              t("system.status.theme.none")
            )}
          </Fact>
        </dl>
        {fallback.length === 0 ? (
          <p className="flex items-center gap-2 text-sm text-muted-foreground">
            <ToneIcon tone="ok" />
            {t("system.status.theme.allServe")}
          </p>
        ) : (
          <div className="flex flex-col gap-1.5">
            <p className="flex items-center gap-2 text-sm text-foreground">
              <ToneIcon tone="warn" />
              {t("system.status.theme.fallback", { count: fallback.length })}
            </p>
            <ul className="ml-6 flex flex-col gap-0.5">
              {fallback.map((n) => (
                <li key={n} className="font-mono text-xs break-all text-muted-foreground">
                  {n}
                </li>
              ))}
            </ul>
            <Link
              to="/system/$tab"
              params={{ tab: "nodes" }}
              className="ml-6 text-sm font-medium text-primary hover:underline"
            >
              {t("system.status.theme.nodes")}
            </Link>
          </div>
        )}
      </div>
    </Panel>
  );
}

function MailPanel({ s }: { s: SystemStatusDto }) {
  const { t } = useTranslation("admin");
  const configured = s.mail.configured;
  const Icon = configured ? CircleCheckIcon : CircleDashedIcon;
  return (
    <Panel title={t("system.status.mail.title")}>
      <div className="flex flex-col gap-3">
        <div className="flex items-start gap-3">
          <Icon
            aria-hidden="true"
            className={cn(
              "mt-0.5 size-4 shrink-0",
              configured ? "text-success" : "text-muted-foreground",
            )}
          />
          <div className="flex min-w-0 flex-col gap-0.5">
            <p className="text-sm font-medium text-foreground">
              {configured
                ? t("system.status.mail.configured")
                : t("system.status.mail.notConfigured")}
            </p>
            <p className="text-sm text-muted-foreground">
              {configured
                ? t("system.status.mail.configuredHint")
                : t("system.status.mail.notConfiguredHint")}
            </p>
          </div>
        </div>
        {configured && s.mail.from ? (
          <dl>
            <Fact label={t("system.status.mail.from")}>
              <span className="font-mono">{s.mail.from}</span>
            </Fact>
          </dl>
        ) : null}
      </div>
    </Panel>
  );
}

function StatusSkeleton() {
  return (
    <div aria-busy="true" className="grid grid-cols-[minmax(0,1fr)] gap-4 md:grid-cols-2">
      {Array.from({ length: 6 }, (_, i) => (
        <div key={i} className="flex flex-col gap-4 rounded-xl border bg-card p-5">
          <Skeleton className="h-5 w-32" />
          <ListSkeleton rows={3} />
        </div>
      ))}
    </div>
  );
}
