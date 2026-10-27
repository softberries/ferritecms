// Nodes (`GET /system/nodes`): one card per registered node, live first: name, live or
// stale, version, schema, started and last heartbeat; what its `meta` holds (H-Y2: an
// older node may lack any member, so only what is present is shown): the theme it serves
// (name, origin, hash, fallback, error), its plugins (version and state, errors), the
// permalink error, the job kinds it runs, and a clock skew over 1 s. Every error is the
// node's scrubbed text, shown as text.

import { useQuery } from "@tanstack/react-query";
import { cn } from "cn";
import { ServerIcon } from "lucide-react";
import { useTranslation } from "react-i18next";
import { INLINE_ERRORS } from "@/api/client";
import { type NodeDto, nodesQuery } from "@/api/nodes";
import { systemStatusQuery } from "@/api/system";
import { EmptyState } from "@/components/kit/empty-state";
import { ErrorState } from "@/components/kit/error-state";
import { RelativeTime } from "@/components/kit/relative-time";
import { Badge } from "@/components/ui/badge";
import { Skeleton } from "@/components/ui/skeleton";
import { skewFlag, themeOriginText } from "./system-model";
import { ExpandableText, Fact, ToneIcon } from "./system-parts";

export function NodesTab() {
  const { t } = useTranslation("admin");
  const nodes = useQuery({ ...nodesQuery, meta: INLINE_ERRORS });
  // Which node answered (the "This node" badge); optional.
  const status = useQuery({ ...systemStatusQuery, meta: INLINE_ERRORS });
  if (nodes.isError) {
    return <ErrorState error={nodes.error} retry={() => void nodes.refetch()} />;
  }
  if (!nodes.data) {
    return (
      <div aria-busy="true" className="grid grid-cols-[minmax(0,1fr)] gap-4 lg:grid-cols-2">
        {[0, 1].map((i) => (
          <div key={i} className="flex flex-col gap-4 rounded-xl border bg-card p-5">
            <Skeleton className="h-5 w-40" />
            <Skeleton className="h-16 w-full" />
            <Skeleton className="h-10 w-full" />
          </div>
        ))}
      </div>
    );
  }
  if (nodes.data.length === 0) {
    return (
      <EmptyState
        icon={ServerIcon}
        title={t("system.nodes.empty.title")}
        body={t("system.nodes.empty.body")}
      />
    );
  }
  const sorted = [...nodes.data].sort(
    (a, b) => Number(b.live) - Number(a.live) || a.name.localeCompare(b.name),
  );
  return (
    <ul className="grid grid-cols-[minmax(0,1fr)] gap-4 lg:grid-cols-2">
      {sorted.map((n) => (
        <li key={n.id} className="min-w-0">
          <NodeCard node={n} self={n.id === status.data?.node.id} />
        </li>
      ))}
    </ul>
  );
}

const PLUGIN_TONE: Record<string, string> = {
  loaded: "bg-success",
  failed: "bg-destructive",
  disabled: "bg-muted-foreground/60",
};

function NodeCard({ node, self }: { node: NodeDto; self: boolean }) {
  const { t } = useTranslation("admin");
  const meta = node.meta ?? {};
  const theme = meta.theme ?? null;
  const plugins = Object.entries(meta.plugins ?? {}).sort(([a], [b]) => a.localeCompare(b));
  const skew = meta.clock_skew_ms;
  const origin = themeOriginText(t, theme?.origin);
  const titleId = `node-${node.id}`;
  const pluginStatus = (s: string | null | undefined) =>
    s === "loaded" || s === "failed" || s === "disabled"
      ? t(`system.nodes.pluginStatus.${s}`)
      : (s ?? t("system.nodes.notReported"));

  return (
    <article
      aria-labelledby={titleId}
      data-slot="node-card"
      data-name={node.name}
      data-live={node.live}
      className="flex min-w-0 flex-col gap-4 rounded-xl border bg-card p-5 text-card-foreground shadow-1"
    >
      <header className="flex flex-wrap items-center gap-x-3 gap-y-2">
        <span
          aria-hidden="true"
          className={cn(
            "size-2.5 shrink-0 rounded-full",
            node.live ? "bg-success" : "bg-muted-foreground/60",
          )}
        />
        <h2 id={titleId} className="min-w-0 font-mono text-md font-semibold break-all">
          {node.name}
        </h2>
        <Badge variant="outline" className={node.live ? "text-success" : "text-muted-foreground"}>
          {node.live ? t("system.nodes.live") : t("system.nodes.stale")}
        </Badge>
        {self ? <Badge variant="secondary">{t("system.nodes.thisNode")}</Badge> : null}
        {skewFlag(skew) && typeof skew === "number" ? (
          <span className="flex items-center gap-1.5 text-sm text-warning">
            <ToneIcon tone="warn" />
            {t(skew > 0 ? "system.nodes.skew.ahead" : "system.nodes.skew.behind", {
              seconds: new Intl.NumberFormat(undefined, { maximumFractionDigits: 1 }).format(
                Math.abs(skew) / 1000,
              ),
            })}
          </span>
        ) : null}
      </header>

      <dl className="grid grid-cols-2 gap-4 sm:grid-cols-4">
        <Fact label={t("system.nodes.version")}>
          <span className="font-mono tabular-nums">{node.version}</span>
        </Fact>
        <Fact label={t("system.nodes.schema")}>
          <span className="tabular-nums">{node.schema_version}</span>
        </Fact>
        <Fact label={t("system.nodes.started")}>
          <RelativeTime at={node.started_at} />
        </Fact>
        <Fact label={t("system.nodes.heartbeat")}>
          <RelativeTime at={node.heartbeat_at} />
        </Fact>
      </dl>

      <section aria-label={t("system.nodes.theme.title")} className="flex flex-col gap-2">
        <h3 className="text-xs font-medium tracking-wide text-muted-foreground uppercase">
          {t("system.nodes.theme.title")}
        </h3>
        {theme ? (
          <div className="flex flex-col gap-1.5">
            <p className="flex flex-wrap items-center gap-2 text-sm">
              {theme.name ? <span className="font-medium">{theme.name}</span> : null}
              {origin ? <span className="text-muted-foreground">{origin}</span> : null}
              {theme.fallback ? (
                <Badge variant="outline" className="text-warning">
                  {t("system.nodes.theme.fallback")}
                </Badge>
              ) : null}
            </p>
            {theme.hash ? (
              <p className="truncate font-mono text-xs text-muted-foreground" title={theme.hash}>
                {t("system.nodes.theme.hash")} {theme.hash}
              </p>
            ) : null}
            {theme.error ? <ExpandableText text={theme.error} /> : null}
          </div>
        ) : (
          <p className="text-sm text-muted-foreground">{t("system.nodes.notReported")}</p>
        )}
      </section>

      {meta.plugins !== undefined || meta.plugins_error ? (
        <section aria-label={t("system.nodes.plugins.title")} className="flex flex-col gap-2">
          <h3 className="text-xs font-medium tracking-wide text-muted-foreground uppercase">
            {t("system.nodes.plugins.title")}
          </h3>
          {meta.plugins_error ? (
            <div className="flex flex-col gap-1">
              <p className="text-sm text-destructive">{t("system.nodes.plugins.error")}</p>
              <ExpandableText text={meta.plugins_error} />
            </div>
          ) : null}
          {plugins.length > 0 ? (
            <ul aria-label={t("system.nodes.plugins.title")} className="flex flex-col divide-y">
              {plugins.map(([name, p]) => (
                <li key={name} className="flex min-w-0 flex-col gap-1 py-1.5">
                  <span className="flex flex-wrap items-center gap-x-2 gap-y-0.5 text-sm">
                    <span
                      aria-hidden="true"
                      className={cn(
                        "size-2 shrink-0 rounded-full",
                        PLUGIN_TONE[p.status ?? ""] ?? "bg-warning",
                      )}
                    />
                    <span className="font-mono break-all">{name}</span>
                    {p.version ? (
                      <span className="text-xs text-muted-foreground tabular-nums">
                        {p.version}
                      </span>
                    ) : null}
                    <span className="ml-auto text-xs text-muted-foreground">
                      {pluginStatus(p.status)}
                    </span>
                  </span>
                  {p.error ? <ExpandableText text={p.error} className="pl-4" /> : null}
                </li>
              ))}
            </ul>
          ) : !meta.plugins_error ? (
            <p className="text-sm text-muted-foreground">{t("system.nodes.plugins.none")}</p>
          ) : null}
        </section>
      ) : null}

      {meta.permalinks_error ? (
        <section aria-label={t("system.nodes.permalinks")} className="flex flex-col gap-1">
          <h3 className="text-xs font-medium tracking-wide text-muted-foreground uppercase">
            {t("system.nodes.permalinks")}
          </h3>
          <ExpandableText text={meta.permalinks_error} />
        </section>
      ) : null}

      {meta.job_kinds ? (
        <section aria-label={t("system.nodes.jobKinds")} className="flex flex-col gap-2">
          <h3 className="text-xs font-medium tracking-wide text-muted-foreground uppercase">
            {t("system.nodes.jobKinds")}
          </h3>
          {meta.job_kinds.length > 0 ? (
            <ul aria-label={t("system.nodes.jobKinds")} className="flex flex-wrap gap-1.5">
              {[...meta.job_kinds].sort().map((k) => (
                <li
                  key={k}
                  className="rounded-md bg-muted px-1.5 font-mono text-xs leading-6 break-all"
                >
                  {k}
                </li>
              ))}
            </ul>
          ) : (
            <p className="text-sm text-muted-foreground">{t("system.nodes.noJobKinds")}</p>
          )}
        </section>
      ) : null}
    </article>
  );
}
