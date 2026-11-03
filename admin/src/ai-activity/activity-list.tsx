// AI activity (design 17 §11; `read_admin`): the change sets as a table — the assistant
// (an app or a personal token), who it acted as (administrators), started, last change,
// changes and status; filters by status, every user's (administrators) and one
// connection (`?agent=`). The empty state explains what will appear here.

import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { SparklesIcon, XIcon } from "lucide-react";
import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import { type ChangeSetDto, changeSetsQuery } from "@/api/change-sets";
import { DataTable } from "@/components/kit/data-table";
import { type DataTableColumn, dataTableColumns } from "@/components/kit/data-table-features";
import { EmptyState } from "@/components/kit/empty-state";
import { ErrorState } from "@/components/kit/error-state";
import { PageHeader } from "@/components/kit/page-header";
import { RelativeTime } from "@/components/kit/relative-time";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Label } from "@/components/ui/label";
import { AgentMark, SetStatusBadge } from "./marks";

export interface ActivitySearch {
  agent?: string;
  all?: true;
  open?: boolean;
}

export function ActivityList({
  search,
  setSearch,
  admin,
}: {
  search: ActivitySearch;
  setSearch: (next: ActivitySearch) => void;
  /** `manage_users`: every user's sets and the user column. */
  admin: boolean;
}) {
  const { t } = useTranslation("ai");
  const navigate = useNavigate();
  const all = admin && search.all === true;
  const sets = useQuery(
    changeSetsQuery({ agent: search.agent, user: all ? "all" : undefined, open: search.open }),
  );
  const columns = useColumns(all);
  const items = sets.data?.items ?? [];
  const open = (s: ChangeSetDto) => void navigate({ to: "/ai-activity/$id", params: { id: s.id } });
  return (
    <div data-slot="ai-activity" className="mx-auto flex w-full max-w-6xl flex-col gap-5">
      <PageHeader title={t("activity.title")} description={t("activity.description")} />
      <div
        role="group"
        aria-label={t("activity.filters.label")}
        className="flex flex-wrap items-center gap-x-5 gap-y-2"
      >
        <div className="flex items-center gap-2">
          <Checkbox
            id="activity-open"
            checked={search.open === true}
            onCheckedChange={(v) => setSearch({ ...search, open: v === true ? true : undefined })}
          />
          <Label htmlFor="activity-open">{t("activity.status.open")}</Label>
        </div>
        {admin ? (
          <div className="flex items-center gap-2">
            <Checkbox
              id="activity-all"
              checked={all}
              onCheckedChange={(v) => setSearch({ ...search, all: v === true ? true : undefined })}
            />
            <Label htmlFor="activity-all">{t("activity.filters.allUsers")}</Label>
          </div>
        ) : null}
        {search.agent ? (
          <Button
            variant="outline"
            onClick={() => setSearch({ ...search, agent: undefined })}
            className="pointer-coarse:min-h-11"
          >
            <XIcon data-icon="inline-start" />
            {t("activity.filters.clear")}
          </Button>
        ) : null}
      </div>
      {sets.isError && !sets.data ? (
        <ErrorState error={sets.error} retry={() => void sets.refetch()} />
      ) : (
        <DataTable
          label={t("activity.list")}
          columns={columns}
          data={items}
          getRowId={(s) => s.id}
          onRowOpen={open}
          loading={sets.isPending}
          columnClassName={{
            client: "w-full max-w-0 md:w-[34%]",
            user: "hidden md:table-cell",
            started: "hidden lg:table-cell",
            last: "hidden sm:table-cell",
            changes: "hidden sm:table-cell",
          }}
          empty={
            <EmptyState
              icon={SparklesIcon}
              title={t("activity.empty.title")}
              body={t("activity.empty.body")}
              className="border-0"
            />
          }
        />
      )}
    </div>
  );
}

function useColumns(withUser: boolean): DataTableColumn<ChangeSetDto>[] {
  const { t } = useTranslation("ai");
  return useMemo(() => {
    const col = dataTableColumns<ChangeSetDto>();
    const cols: DataTableColumn<ChangeSetDto>[] = [
      col.accessor((s) => s.agent.client, {
        id: "client",
        header: () => t("activity.columns.client"),
        cell: (c) => {
          const s = c.row.original;
          return (
            <div className="flex min-w-0 items-center gap-3 py-2">
              <AgentMark agent={s.agent} />
              <div className="flex min-w-0 flex-col gap-0.5">
                <span data-slot="set-client" className="truncate font-medium text-foreground">
                  {s.agent.client}
                </span>
                <span className="truncate text-xs text-muted-foreground">
                  {s.agent.kind === "token" ? t("activity.token") : null}
                  {s.agent.connected ? null : ` ${t("activity.disconnected")}`}
                </span>
              </div>
            </div>
          );
        },
      }),
    ];
    if (withUser) {
      cols.push(
        col.display({
          id: "user",
          header: () => t("activity.columns.user"),
          cell: (c) => (
            <span className="truncate text-sm text-muted-foreground">
              {c.row.original.user?.display_name ?? t("activity.deletedUser")}
            </span>
          ),
        }),
      );
    }
    cols.push(
      col.display({
        id: "started",
        header: () => t("activity.columns.started"),
        cell: (c) => (
          <RelativeTime at={c.row.original.started_at} className="text-sm text-muted-foreground" />
        ),
      }),
      col.display({
        id: "last",
        header: () => t("activity.columns.last"),
        cell: (c) => (
          <RelativeTime at={c.row.original.last_at} className="text-sm text-muted-foreground" />
        ),
      }),
      col.display({
        id: "changes",
        header: () => t("activity.columns.changes"),
        cell: (c) => (
          <span className="text-sm text-muted-foreground tabular-nums">
            {t("activity.changes", { count: c.row.original.changes })}
          </span>
        ),
      }),
      col.display({
        id: "status",
        header: () => t("activity.columns.status"),
        cell: (c) => <SetStatusBadge set={c.row.original} />,
      }),
    );
    return cols;
  }, [t, withUser]);
}
