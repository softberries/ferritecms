// The review queue (`/admin/reviews`, `g v`; design 15 §11.4): tabs "Assigned to me", "All
// open" (editors and admins: `edit_others`) and "Submitted by me", oldest first. A row opens
// the editor (an item review) or the release (a staged change's review); the Review
// section is in the editor's side panel.

import { useQuery } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import { ClipboardCheckIcon } from "lucide-react";
import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import { type QueueTab, type ReviewDto, reviewsQuery } from "@/api/reviews";
import { useMe } from "@/auth/session";
import { DataTable } from "@/components/kit/data-table";
import { type DataTableColumn, dataTableColumns } from "@/components/kit/data-table-features";
import { EmptyState } from "@/components/kit/empty-state";
import { ErrorState } from "@/components/kit/error-state";
import { PageHeader } from "@/components/kit/page-header";
import { RelativeTime } from "@/components/kit/relative-time";
import { ScrollFade } from "@/components/kit/scroll-fade";
import { TAB_LIST, tabCountClass, tabLinkClass } from "@/components/kit/tab-styles";
import { ReviewStatus } from "./review-status";

export function ReviewQueue({ tab }: { tab: QueueTab }) {
  const { t } = useTranslation("reviews");
  const navigate = useNavigate();
  const me = useMe();
  const editor = me?.capabilities.includes("edit_others") === true;
  const tabs: QueueTab[] = editor ? ["assigned", "open", "mine"] : ["assigned", "mine"];
  const current = tabs.includes(tab) ? tab : "assigned";
  const list = useQuery(reviewsQuery(current));
  const columns = useColumns();
  const open = (r: ReviewDto) => {
    if (r.target_kind === "entry" && r.release_id) {
      void navigate({ to: "/releases/$id", params: { id: r.release_id } });
    } else {
      void navigate({
        to: "/content/$type/$id",
        params: { type: r.item_type ?? "post", id: r.item_id },
      });
    }
  };

  return (
    <div data-slot="reviews-screen" className="mx-auto flex w-full max-w-6xl flex-col gap-5">
      <PageHeader
        title={t("title")}
        description={t("description")}
        tabs={
          <nav aria-label={t("tabs.label")}>
            <ScrollFade>
              <ul className={TAB_LIST}>
                {tabs.map((tb) => (
                  <li key={tb}>
                    <Link
                      to="/reviews"
                      search={{ tab: tb }}
                      aria-current={current === tb ? "page" : undefined}
                      className={tabLinkClass(current === tb)}
                    >
                      {t(`tabs.${tb}`)}
                      {tb === "assigned" && list.data && current === "assigned" ? (
                        <span className={tabCountClass(true)}>{list.data.assigned_to_me}</span>
                      ) : null}
                    </Link>
                  </li>
                ))}
              </ul>
            </ScrollFade>
          </nav>
        }
      />
      {list.isError && !list.data ? (
        <ErrorState error={list.error} retry={() => void list.refetch()} />
      ) : (
        <DataTable
          label={t("list")}
          columns={columns}
          data={list.data?.items ?? []}
          getRowId={(r) => r.id}
          onRowOpen={open}
          loading={list.isPending}
          columnClassName={{
            item: "w-full max-w-0 md:w-[40%]",
            step: "hidden sm:table-cell",
            submitted: "hidden md:table-cell",
          }}
          empty={
            <EmptyState
              icon={ClipboardCheckIcon}
              title={t(`empty.${current}`)}
              body={t("empty.body")}
              className="border-0"
            />
          }
        />
      )}
    </div>
  );
}

function useColumns(): DataTableColumn<ReviewDto>[] {
  const { t } = useTranslation("reviews");
  return useMemo(() => {
    const col = dataTableColumns<ReviewDto>();
    return [
      col.display({
        id: "item",
        header: () => t("columns.item"),
        cell: (c) => {
          const r = c.row.original;
          return (
            <div className="flex min-w-0 flex-col gap-0.5 py-2">
              <span data-slot="review-item" className="truncate font-medium text-foreground">
                {r.item_title || t("untitled")}
              </span>
              <span className="truncate text-xs text-muted-foreground">
                {r.target_kind === "entry" ? t("entryOf") : r.item_type}
              </span>
            </div>
          );
        },
      }),
      col.display({
        id: "step",
        header: () => t("columns.step"),
        cell: (c) => {
          const r = c.row.original;
          return (
            <span className="text-sm text-muted-foreground">{r.steps[r.step]?.label ?? "—"}</span>
          );
        },
      }),
      col.accessor("status", {
        id: "status",
        header: () => t("columns.status"),
        cell: (c) => <ReviewStatus status={c.row.original.status} />,
      }),
      col.accessor("submitted_at", {
        id: "submitted",
        header: () => t("columns.submitted"),
        cell: (c) => (
          <RelativeTime
            at={c.row.original.submitted_at}
            className="text-sm text-muted-foreground"
          />
        ),
      }),
    ];
  }, [t]);
}
