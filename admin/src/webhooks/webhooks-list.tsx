// The Webhooks screen (design 18 §10.1; `manage_webhooks`): every endpoint with its name,
// URL host (the full URL in the tooltip), events, state, last delivery and the last 24
// hours' counts; a banner when one was disabled automatically or is paused; New webhook.

import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { PlusIcon, TriangleAlertIcon, WebhookIcon } from "lucide-react";
import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import { type WebhookDto, webhooksQuery } from "@/api/webhooks";
import { DataTable } from "@/components/kit/data-table";
import { type DataTableColumn, dataTableColumns } from "@/components/kit/data-table-features";
import { EmptyState } from "@/components/kit/empty-state";
import { ErrorState } from "@/components/kit/error-state";
import { PageHeader } from "@/components/kit/page-header";
import { RelativeTime } from "@/components/kit/relative-time";
import { Button } from "@/components/ui/button";
import { NewWebhookDialog } from "./new-webhook-dialog";
import { DisabledReason, WebhookStateBadge } from "./state-badge";
import { eventsSummary, hostOf, needsAttention } from "./webhooks-model";

export interface WebhooksScreenProps {
  /** The New webhook dialog is open (`?new=true`, the palette's "New webhook"). */
  creating: boolean;
  setCreating: (open: boolean) => void;
}

/** The later of the last success and the last failure. */
function lastDelivery(w: WebhookDto): string | null {
  const s = w.last_success_at;
  const f = w.last_failure_at;
  if (s && f) return s > f ? s : f;
  return s ?? f ?? null;
}

export function WebhooksScreen({ creating, setCreating }: WebhooksScreenProps) {
  const { t } = useTranslation("webhooks");
  const navigate = useNavigate();
  const hooks = useQuery(webhooksQuery);
  const list = hooks.data ?? [];
  const attention = needsAttention(list);
  const columns = useColumns();
  const open = (w: WebhookDto) => void navigate({ to: "/webhooks/$id", params: { id: w.id } });

  return (
    <div data-slot="webhooks-screen" className="mx-auto flex w-full max-w-6xl flex-col gap-5">
      <PageHeader
        title={t("title")}
        description={t("description")}
        actions={
          <Button onClick={() => setCreating(true)}>
            <PlusIcon data-icon="inline-start" />
            {t("new")}
          </Button>
        }
      />
      {attention.disabled > 0 || attention.paused > 0 ? (
        <div
          role="status"
          data-slot="webhooks-banner"
          className="flex items-start gap-2.5 rounded-lg border border-warning/50 bg-warning/8 px-3 py-2.5 text-sm text-foreground"
        >
          <TriangleAlertIcon aria-hidden="true" className="mt-0.5 size-4 shrink-0 text-warning" />
          <div className="flex flex-col gap-1">
            {attention.disabled > 0 ? (
              <p>{t("banner.disabled", { count: attention.disabled })}</p>
            ) : null}
            {attention.paused > 0 ? <p>{t("banner.paused", { count: attention.paused })}</p> : null}
          </div>
        </div>
      ) : null}
      {hooks.isError && !hooks.data ? (
        <ErrorState error={hooks.error} retry={() => void hooks.refetch()} />
      ) : (
        <DataTable
          label={t("list")}
          columns={columns}
          data={list}
          getRowId={(w) => w.id}
          onRowOpen={open}
          loading={hooks.isPending}
          columnClassName={{
            webhook: "w-full max-w-0 md:w-[34%]",
            events: "hidden md:table-cell",
            state: "hidden sm:table-cell",
            last: "hidden lg:table-cell",
            counts: "hidden lg:table-cell",
          }}
          empty={
            <EmptyState
              icon={WebhookIcon}
              title={t("empty.title")}
              body={t("empty.body")}
              action={
                <Button variant="outline" onClick={() => setCreating(true)}>
                  {t("new")}
                </Button>
              }
              className="border-0"
            />
          }
        />
      )}
      <NewWebhookDialog open={creating} onOpenChange={setCreating} />
    </div>
  );
}

function useColumns(): DataTableColumn<WebhookDto>[] {
  const { t } = useTranslation("webhooks");
  return useMemo(() => {
    const col = dataTableColumns<WebhookDto>();
    return [
      col.accessor("name", {
        id: "webhook",
        header: () => t("columns.webhook"),
        cell: (c) => {
          const w = c.row.original;
          return (
            <div className="flex min-w-0 flex-col gap-0.5 py-2">
              <span data-slot="webhook-name" className="truncate font-medium text-foreground">
                {w.name}
              </span>
              <span
                data-slot="webhook-host"
                className="truncate font-mono text-xs text-muted-foreground"
                title={w.url}
              >
                {hostOf(w.url)}
              </span>
              <span className="mt-1 sm:hidden">
                <WebhookStateBadge hook={w} />
              </span>
            </div>
          );
        },
      }),
      col.accessor("events", {
        id: "events",
        header: () => t("columns.events"),
        cell: (c) => (
          <span className="line-clamp-2 text-sm whitespace-normal text-muted-foreground">
            {eventsSummary(t, c.row.original.events)}
          </span>
        ),
      }),
      col.accessor("state", {
        id: "state",
        header: () => t("columns.state"),
        cell: (c) => (
          <div className="flex min-w-0 flex-col items-start gap-1">
            <WebhookStateBadge hook={c.row.original} />
            <DisabledReason hook={c.row.original} />
          </div>
        ),
      }),
      col.display({
        id: "last",
        header: () => t("columns.last"),
        cell: (c) => {
          const at = lastDelivery(c.row.original);
          return at ? (
            <RelativeTime at={at} className="text-sm text-muted-foreground" />
          ) : (
            <span className="text-sm text-muted-foreground">{t("never")}</span>
          );
        },
      }),
      col.display({
        id: "counts",
        header: () => t("columns.counts"),
        cell: (c) => {
          const n = c.row.original.counts_24h;
          return (
            <span className="text-sm text-muted-foreground tabular-nums">
              {t("counts", { delivered: n.delivered, failed: n.failed })}
            </span>
          );
        },
      }),
    ];
  }, [t]);
}
