// The delivery log of one endpoint (design 18 §10.1): status filter, newest first with
// Load more; a row opens a sheet with the event's JSON (read-only), every attempt (code,
// duration, error and the response excerpt as plain text) and Redeliver.

import { useInfiniteQuery, useQuery, useQueryClient } from "@tanstack/react-query";
import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { reportApiError } from "@/api/client";
import { qk } from "@/api/keys";
import {
  type DeliveryDto,
  type DeliveryStatus,
  deliveriesQuery,
  deliveryQuery,
  redeliver,
} from "@/api/webhooks";
import { DataTable } from "@/components/kit/data-table";
import { type DataTableColumn, dataTableColumns } from "@/components/kit/data-table-features";
import { ErrorState } from "@/components/kit/error-state";
import { RelativeTime } from "@/components/kit/relative-time";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";
import { problemText } from "@/structure/taxonomies/terms-model";

const STATUSES: DeliveryStatus[] = ["pending", "delivered", "failed", "skipped"];

const TONE: Record<DeliveryStatus, string> = {
  pending: "text-muted-foreground",
  delivered: "text-success",
  failed: "text-destructive",
  skipped: "text-warning",
};

export function DeliveryStatusText({ status }: { status: DeliveryStatus }) {
  const { t } = useTranslation("webhooks");
  return (
    <span
      data-slot="delivery-status"
      data-status={status}
      className={`inline-flex items-center gap-1.5 text-sm font-medium ${TONE[status]}`}
    >
      <span
        aria-hidden="true"
        className={`size-2 rounded-full border-[1.5px] border-current ${status === "pending" ? "bg-transparent" : "bg-current"}`}
      />
      {t(`deliveries.statuses.${status}`)}
    </span>
  );
}

export function DeliveriesTable({ id, canRedeliver }: { id: string; canRedeliver: boolean }) {
  const { t } = useTranslation("webhooks");
  const [status, setStatus] = useState<DeliveryStatus | undefined>(undefined);
  const [open, setOpen] = useState<string | null>(null);
  const pages = useInfiniteQuery(deliveriesQuery(id, status));
  const rows = useMemo(() => pages.data?.pages.flatMap((p) => p.items) ?? [], [pages.data]);
  const columns = useColumns();

  return (
    <div data-slot="deliveries" className="flex min-w-0 flex-col gap-3">
      <div className="flex items-center gap-3">
        <span className="text-sm text-muted-foreground">{t("deliveries.status")}</span>
        <Select
          value={status ?? "all"}
          onValueChange={(v) => setStatus(v === "all" ? undefined : (v as DeliveryStatus))}
        >
          <SelectTrigger aria-label={t("deliveries.status")} className="w-44">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="all">{t("deliveries.all")}</SelectItem>
            {STATUSES.map((s) => (
              <SelectItem key={s} value={s}>
                {t(`deliveries.statuses.${s}`)}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
      {pages.isError && !pages.data ? (
        <ErrorState error={pages.error} retry={() => void pages.refetch()} />
      ) : (
        <DataTable
          label={t("deliveries.label")}
          columns={columns}
          data={rows}
          getRowId={(d) => d.id}
          onRowOpen={(d) => setOpen(d.id)}
          loading={pages.isPending}
          columnClassName={{
            event: "w-full max-w-0",
            created: "hidden sm:table-cell",
            attempts: "hidden md:table-cell text-right",
            code: "hidden md:table-cell",
            next: "hidden lg:table-cell",
          }}
          empty={
            <p className="px-4 py-8 text-center text-sm text-muted-foreground">
              {t("deliveries.empty")}
            </p>
          }
        />
      )}
      {pages.hasNextPage ? (
        <Button
          variant="outline"
          className="self-center"
          disabled={pages.isFetchingNextPage}
          onClick={() => void pages.fetchNextPage()}
        >
          {pages.isFetchingNextPage ? t("deliveries.loadingMore") : t("deliveries.loadMore")}
        </Button>
      ) : null}
      <DeliverySheet
        id={id}
        delivery={open}
        canRedeliver={canRedeliver}
        onClose={() => setOpen(null)}
        onRedelivered={(next) => setOpen(next)}
      />
    </div>
  );
}

function useColumns(): DataTableColumn<DeliveryDto>[] {
  const { t } = useTranslation("webhooks");
  return useMemo(() => {
    const col = dataTableColumns<DeliveryDto>();
    return [
      col.accessor("event_type", {
        id: "event",
        header: () => t("deliveries.columns.event"),
        cell: (c) => (
          <div className="flex min-w-0 flex-col gap-0.5 py-1.5">
            <span className="truncate font-mono text-[0.8125rem] text-foreground">
              {c.row.original.event_type}
            </span>
            {c.row.original.last_error ? (
              <span className="truncate text-xs text-muted-foreground">
                {c.row.original.last_error}
              </span>
            ) : null}
          </div>
        ),
      }),
      col.accessor("status", {
        id: "status",
        header: () => t("deliveries.columns.status"),
        cell: (c) => <DeliveryStatusText status={c.row.original.status} />,
      }),
      col.accessor("created_at", {
        id: "created",
        header: () => t("deliveries.columns.created"),
        cell: (c) => (
          <RelativeTime at={c.row.original.created_at} className="text-sm text-muted-foreground" />
        ),
      }),
      col.accessor("attempts", {
        id: "attempts",
        header: () => <span className="block text-right">{t("deliveries.columns.attempts")}</span>,
        cell: (c) => (
          <span className="block text-right tabular-nums">{c.row.original.attempts}</span>
        ),
      }),
      col.accessor("last_status_code", {
        id: "code",
        header: () => t("deliveries.columns.code"),
        cell: (c) => <span className="tabular-nums">{c.row.original.last_status_code ?? "–"}</span>,
      }),
      col.accessor("next_attempt_at", {
        id: "next",
        header: () => t("deliveries.columns.next"),
        cell: (c) =>
          c.row.original.status === "pending" && c.row.original.next_attempt_at ? (
            <RelativeTime
              at={c.row.original.next_attempt_at}
              className="text-sm text-muted-foreground"
            />
          ) : (
            <span className="text-muted-foreground">–</span>
          ),
      }),
    ];
  }, [t]);
}

function DeliverySheet({
  id,
  delivery,
  canRedeliver,
  onClose,
  onRedelivered,
}: {
  id: string;
  delivery: string | null;
  canRedeliver: boolean;
  onClose: () => void;
  onRedelivered: (delivery: string) => void;
}) {
  const { t } = useTranslation("webhooks");
  const queryClient = useQueryClient();
  const detail = useQuery({
    ...deliveryQuery(id, delivery ?? ""),
    enabled: delivery !== null,
  });
  const [pending, setPending] = useState(false);
  const d = detail.data;
  const again = async () => {
    if (!delivery) return;
    setPending(true);
    try {
      const next = await redeliver(id, delivery);
      toast.success(t("sheet.redelivered"));
      void queryClient.invalidateQueries({ queryKey: qk.webhooks.item(id) });
      onRedelivered(next);
    } catch (error) {
      reportApiError(error);
      toast.error(problemText(error));
    } finally {
      setPending(false);
    }
  };
  return (
    <Sheet open={delivery !== null} onOpenChange={(o) => !o && onClose()}>
      <SheetContent data-slot="delivery-sheet" className="w-full overflow-y-auto sm:max-w-xl">
        <SheetHeader>
          <SheetTitle>{d ? d.event.type : t("sheet.title")}</SheetTitle>
          <SheetDescription>
            {d ? (
              <span className="flex flex-wrap items-center gap-3">
                <DeliveryStatusText status={d.delivery.status} />
                <RelativeTime at={d.delivery.created_at} />
                {d.delivery.redelivery_of ? <span>{t("sheet.redeliveryOf")}</span> : null}
              </span>
            ) : null}
          </SheetDescription>
        </SheetHeader>
        {d ? (
          <div className="flex min-w-0 flex-col gap-6 px-4 pb-6">
            <section className="flex flex-col gap-2">
              <h3 className="text-sm font-medium text-foreground">{t("sheet.attempts")}</h3>
              {d.attempts.length === 0 ? (
                <p className="text-sm text-muted-foreground">{t("sheet.noAttempts")}</p>
              ) : (
                <ol className="flex flex-col divide-y rounded-lg border">
                  {d.attempts.map((a) => (
                    <li key={a.attempt} data-slot="attempt" className="flex flex-col gap-1.5 p-3">
                      <div className="flex flex-wrap items-baseline justify-between gap-2 text-sm">
                        <span className="font-medium">{t("sheet.attempt", { n: a.attempt })}</span>
                        <span className="tabular-nums text-muted-foreground">
                          {a.status_code ?? t("sheet.noAnswer")} ·{" "}
                          {t("sheet.duration", { ms: a.duration_ms })}
                        </span>
                      </div>
                      {a.error ? <p className="text-sm text-destructive">{a.error}</p> : null}
                      {a.response_excerpt ? (
                        <pre
                          data-slot="response-excerpt"
                          aria-label={t("sheet.excerpt")}
                          className="max-h-40 overflow-auto rounded-md bg-muted px-2 py-1.5 font-mono text-xs whitespace-pre-wrap break-words"
                        >
                          {a.response_excerpt}
                        </pre>
                      ) : null}
                    </li>
                  ))}
                </ol>
              )}
            </section>
            <section className="flex flex-col gap-2">
              <h3 className="text-sm font-medium text-foreground">{t("sheet.event")}</h3>
              <pre
                data-slot="event-json"
                tabIndex={0}
                className="max-h-80 overflow-auto rounded-md bg-muted px-3 py-2 font-mono text-xs"
              >
                {JSON.stringify(
                  {
                    id: d.event.id,
                    type: d.event.type,
                    timestamp: d.event.timestamp,
                    data: d.event.data,
                  },
                  null,
                  2,
                )}
              </pre>
            </section>
            {canRedeliver ? (
              <Button
                className="self-start"
                variant="outline"
                disabled={pending}
                aria-busy={pending || undefined}
                onClick={() => void again()}
              >
                {t("sheet.redeliver")}
              </Button>
            ) : null}
          </div>
        ) : null}
      </SheetContent>
    </Sheet>
  );
}
