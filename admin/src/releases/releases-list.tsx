// The Releases screen (`/admin/releases`, `g r`; design 15 §11.3): the releases the viewer
// may see (their own, or all with `edit_others`), filtered by status (Open, Scheduled,
// Applied, Failed), with their entry count, when they go live, who made them and when
// they changed; New release.

import { useQuery } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import { PackageIcon, PlusIcon } from "lucide-react";
import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import {
  RELEASE_STATUSES,
  type ReleaseDto,
  type ReleaseStatus,
  releasesQuery,
} from "@/api/releases";
import { useMe } from "@/auth/session";
import { DataTable } from "@/components/kit/data-table";
import { type DataTableColumn, dataTableColumns } from "@/components/kit/data-table-features";
import { EmptyState } from "@/components/kit/empty-state";
import { ErrorState } from "@/components/kit/error-state";
import { PageHeader } from "@/components/kit/page-header";
import { RelativeTime } from "@/components/kit/relative-time";
import { ScrollFade } from "@/components/kit/scroll-fade";
import { TAB_LIST, tabLinkClass } from "@/components/kit/tab-styles";
import { Button } from "@/components/ui/button";
import { formatWhen } from "@/editor/panel/publishing";
import { NewReleaseDialog } from "./new-release-dialog";
import { ReleaseBadge } from "./release-badge";

export type ReleaseFilter = ReleaseStatus | "all";

export interface ReleasesScreenProps {
  filter: ReleaseFilter;
  creating: boolean;
  setCreating: (open: boolean) => void;
}

export function ReleasesScreen({ filter, creating, setCreating }: ReleasesScreenProps) {
  const { t } = useTranslation("releases");
  const navigate = useNavigate();
  const releases = useQuery(releasesQuery(filter));
  const columns = useColumns();
  const list = releases.data ?? [];

  return (
    <div data-slot="releases-screen" className="mx-auto flex w-full max-w-6xl flex-col gap-5">
      <PageHeader
        title={t("title")}
        description={t("description")}
        actions={
          <Button onClick={() => setCreating(true)}>
            <PlusIcon data-icon="inline-start" />
            {t("new")}
          </Button>
        }
        tabs={
          <nav aria-label={t("filters.label")}>
            <ScrollFade>
              <ul className={TAB_LIST}>
                {(["all", ...RELEASE_STATUSES] as const).map((s) => (
                  <li key={s}>
                    <Link
                      to="/releases"
                      search={s === "all" ? {} : { status: s }}
                      aria-current={filter === s ? "page" : undefined}
                      className={tabLinkClass(filter === s)}
                    >
                      {t(`filters.${s}`)}
                    </Link>
                  </li>
                ))}
              </ul>
            </ScrollFade>
          </nav>
        }
      />
      {releases.isError && !releases.data ? (
        <ErrorState error={releases.error} retry={() => void releases.refetch()} />
      ) : (
        <DataTable
          label={t("list")}
          columns={columns}
          data={list}
          getRowId={(r) => r.id}
          onRowOpen={(r) => void navigate({ to: "/releases/$id", params: { id: r.id } })}
          loading={releases.isPending}
          columnClassName={{
            release: "w-full max-w-0 md:w-[36%]",
            entries: "hidden sm:table-cell",
            scheduled: "hidden md:table-cell",
            creator: "hidden lg:table-cell",
            updated: "hidden lg:table-cell",
          }}
          empty={
            <EmptyState
              icon={PackageIcon}
              title={t("empty.title")}
              body={filter === "all" ? t("empty.body") : t("empty.filtered")}
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
      <NewReleaseDialog
        open={creating}
        onOpenChange={setCreating}
        onCreated={(r) => void navigate({ to: "/releases/$id", params: { id: r.id } })}
      />
    </div>
  );
}

function useColumns(): DataTableColumn<ReleaseDto>[] {
  const { t } = useTranslation("releases");
  const me = useMe();
  const myId = me?.id;
  return useMemo(() => {
    const col = dataTableColumns<ReleaseDto>();
    return [
      col.accessor("title", {
        id: "release",
        header: () => t("columns.release"),
        cell: (c) => {
          const r = c.row.original;
          return (
            <div className="flex min-w-0 flex-col gap-0.5 py-2">
              <span data-slot="release-title" className="truncate font-medium text-foreground">
                {r.title}
              </span>
              {r.description ? (
                <span className="truncate text-xs text-muted-foreground">{r.description}</span>
              ) : null}
            </div>
          );
        },
      }),
      col.accessor("status", {
        id: "status",
        header: () => t("columns.status"),
        cell: (c) => <ReleaseBadge status={c.row.original.status} />,
      }),
      col.accessor("entry_count", {
        id: "entries",
        header: () => t("columns.entries"),
        cell: (c) => (
          <span className="text-sm text-muted-foreground tabular-nums">
            {t("entriesCount", { count: c.row.original.entry_count })}
          </span>
        ),
      }),
      col.display({
        id: "scheduled",
        header: () => t("columns.scheduled"),
        cell: (c) => {
          const r = c.row.original;
          return (
            <span className="text-sm text-muted-foreground tabular-nums">
              {r.status === "scheduled" && r.scheduled_at
                ? formatWhen(r.scheduled_at)
                : t("notScheduled")}
            </span>
          );
        },
      }),
      col.display({
        id: "creator",
        header: () => t("columns.creator"),
        cell: (c) => (
          <span className="text-sm text-muted-foreground">
            {c.row.original.created_by === myId ? t("you") : t("someone")}
          </span>
        ),
      }),
      col.accessor("updated_at", {
        id: "updated",
        header: () => t("columns.updated"),
        cell: (c) => (
          <RelativeTime at={c.row.original.updated_at} className="text-sm text-muted-foreground" />
        ),
      }),
    ];
  }, [t, myId]);
}
