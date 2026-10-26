// The content list's columns and row actions (07 §3). Title with the slug under it in
// mono (pre-flight F6: summaries carry no permalink; a hierarchical type prefixes the
// parent's slug once the parent is known); parent (hierarchical types only); status
// with the scheduled time; author; date (published, else last updated). Below `md` the
// status and date fold into a line under the title, so a phone shows title, meta and
// the actions menu without scrolling sideways.
// The row actions are in `row-actions.tsx`.

import { Link } from "@tanstack/react-router";
import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import { isContentStatus } from "@/api/content";
import type { SchemaContentSummaryDto } from "@/api/schema";
import { dataTableColumns } from "@/components/kit/data-table-features";
import { RelativeTime } from "@/components/kit/relative-time";
import { StatusDot } from "@/components/kit/status-badge";
import { StatusCell, UnavailableParent } from "./list-cells";
import type { ParentState } from "./parent-ref";

export type ContentSummary = SchemaContentSummaryDto;

/** A row action the list carries out (the route owns the requests and the undo). */
export type RowAction = "trash" | "restore" | "delete";

/** The admin path of an item's editor (Tasks 14–15). */
export function editPath(type: string, id: string): string {
  return `/content/${type}/${id}`;
}

/** The date the list shows: published for items that have been, else last updated. */
export function shownDate(item: ContentSummary): { at: string; kind: "published" | "updated" } {
  if (item.published_at && (item.status === "published" || item.status === "private")) {
    return { at: item.published_at, kind: "published" };
  }
  return { at: item.updated_at, kind: "updated" };
}

function initials(name: string): string {
  const parts = name.trim().split(/\s+/).filter(Boolean);
  const letters = parts.length > 1 ? [parts[0], parts[parts.length - 1]] : [parts[0]];
  return letters
    .map((p) => p?.[0] ?? "")
    .join("")
    .toUpperCase();
}

export interface ListColumnsOptions {
  type: string;
  hierarchical: boolean;
  /** Parents by id: from the loaded rows, or resolved one by one (`parent-ref.ts`). */
  parents: ReadonlyMap<string, ParentState>;
}

export function useListColumns({ type, hierarchical, parents }: ListColumnsOptions) {
  const { t } = useTranslation("content");
  const { t: tc } = useTranslation();
  return useMemo(() => {
    const col = dataTableColumns<ContentSummary>();
    const columns = [
      col.accessor("title", {
        header: () => t("columns.title"),
        cell: (c) => {
          const item = c.row.original;
          const parent = item.parent_id ? parents.get(item.parent_id) : undefined;
          const slug = parent?.kind === "found" ? `${parent.ref.slug}/${item.slug}` : item.slug;
          const title = item.title || t("row.untitled");
          const date = shownDate(item);
          return (
            <div className="flex min-w-0 flex-col gap-0.5 py-2">
              <Link
                to={editPath(type, item.id) as "/"}
                tabIndex={-1}
                className="line-clamp-2 max-w-[calc(100vw-10rem)] font-medium whitespace-normal md:whitespace-nowrap text-foreground hover:underline sm:max-w-sm md:block md:truncate lg:max-w-md xl:max-w-xl"
                title={title}
              >
                {title}
              </Link>
              <span
                className="block max-w-[calc(100vw-10rem)] truncate font-mono text-xs text-muted-foreground sm:max-w-sm lg:max-w-md"
                title={slug}
              >
                {slug}
              </span>
              {/* Below `md` the status and date live here (their columns are hidden). */}
              <span className="flex items-center gap-1.5 text-xs text-muted-foreground md:hidden">
                {isContentStatus(item.status) ? (
                  <>
                    <StatusDot status={item.status} />
                    {tc(`status.${item.status}`)}
                    <span aria-hidden="true">·</span>
                  </>
                ) : null}
                <RelativeTime at={date.at} />
              </span>
            </div>
          );
        },
      }),
      ...(hierarchical
        ? [
            col.accessor("parent_id", {
              id: "parent",
              header: () => t("columns.parent"),
              cell: (c) => {
                const id = c.getValue();
                if (!id) {
                  return (
                    <span className="text-muted-foreground">
                      <span aria-hidden="true">—</span>
                      <span className="sr-only">{t("row.noParent")}</span>
                    </span>
                  );
                }
                const parent = parents.get(id);
                if (parent?.kind === "found") {
                  return (
                    <span className="block max-w-48 truncate" title={parent.ref.title}>
                      {parent.ref.title || t("row.untitled")}
                    </span>
                  );
                }
                if (parent?.kind === "unavailable") return <UnavailableParent />;
                return (
                  <span className="text-muted-foreground">
                    <span aria-hidden="true">…</span>
                    <span className="sr-only">{t("row.parentLoading")}</span>
                  </span>
                );
              },
            }),
          ]
        : []),
      col.accessor("status", {
        header: () => t("columns.status"),
        cell: (c) => <StatusCell item={c.row.original} />,
      }),
      col.accessor("author", {
        header: () => t("columns.author"),
        cell: (c) => {
          const name = c.getValue().display_name || c.getValue().login;
          return (
            <span className="flex min-w-0 items-center gap-2">
              <span
                aria-hidden="true"
                className="flex size-6 shrink-0 items-center justify-center rounded-full bg-muted text-[0.625rem] font-semibold text-muted-foreground"
              >
                {initials(name)}
              </span>
              <span className="max-w-36 truncate" title={name}>
                {name}
              </span>
            </span>
          );
        },
      }),
      col.display({
        id: "date",
        header: () => t("columns.date"),
        cell: (c) => {
          const date = shownDate(c.row.original);
          return (
            <span className="flex flex-col py-1.5">
              <RelativeTime at={date.at} className="whitespace-nowrap" />
              <span className="text-xs text-muted-foreground">{t(`row.${date.kind}`)}</span>
            </span>
          );
        },
      }),
    ];
    return col.columns(columns);
  }, [type, hierarchical, parents, t, tc]);
}

/** Which columns hide at which width (`DataTable.columnClassName`). */
export const COLUMN_CLASSES: Record<string, string> = {
  parent: "hidden lg:table-cell",
  status: "hidden md:table-cell",
  author: "hidden lg:table-cell",
  date: "hidden md:table-cell",
};
