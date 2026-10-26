// The library's list view: the M2b-1 `DataTable` with a thumbnail, the title (and the
// original file name under it), type, size, dimensions, who uploaded it and when. Below
// `md` the type and size move under the title and the other columns hide.

import { useMemo, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import type { MediaDto } from "@/api/media";
import { DataTable } from "@/components/kit/data-table";
import {
  type DataTableColumn,
  type RowSelectionState,
  dataTableColumns,
} from "@/components/kit/data-table-features";
import { MediaThumb } from "@/components/kit/media-thumb";
import { RelativeTime } from "@/components/kit/relative-time";
import { authorName } from "./author";
import { dimensions, displayTitle, formatBytes, typeLabel } from "./format";

const COLUMN_CLASSES: Record<string, string> = {
  thumb: "w-14 pr-0",
  type: "hidden md:table-cell",
  size: "hidden md:table-cell text-right",
  dimensions: "hidden lg:table-cell",
  author: "hidden lg:table-cell",
  uploaded: "hidden md:table-cell",
};

export interface MediaTableProps {
  items: MediaDto[];
  label: string;
  loading: boolean;
  empty: ReactNode;
  footer?: ReactNode;
  selection: { state: RowSelectionState; onChange: (next: RowSelectionState) => void };
  onOpen: (media: MediaDto) => void;
  className?: string;
}

export function MediaTable({
  items,
  label,
  loading,
  empty,
  footer,
  selection,
  onOpen,
  className,
}: MediaTableProps) {
  const columns = useColumns();
  return (
    <DataTable
      label={label}
      columns={columns}
      data={items}
      getRowId={(m) => m.id}
      selection={selection}
      onRowOpen={onOpen}
      loading={loading}
      empty={empty}
      footer={footer}
      animateRows
      columnClassName={COLUMN_CLASSES}
      {...(className ? { className } : {})}
    />
  );
}

function useColumns(): DataTableColumn<MediaDto>[] {
  const { t } = useTranslation("media");
  return useMemo(() => {
    const col = dataTableColumns<MediaDto>();
    return col.columns([
      col.display({
        id: "thumb",
        header: () => <span className="sr-only">{t("columns.preview")}</span>,
        cell: (c) => (
          <MediaThumb media={c.row.original} size="sm" caption={false} className="py-1.5" />
        ),
      }),
      col.accessor("title", {
        header: () => t("columns.title"),
        cell: (c) => {
          const item = c.row.original;
          const title = displayTitle(item);
          return (
            <div className="flex min-w-0 flex-col gap-0.5 py-2">
              <span
                className="block max-w-[calc(100vw-11rem)] truncate font-medium text-foreground sm:max-w-sm lg:max-w-md"
                title={title}
              >
                {title}
              </span>
              <span
                className="block max-w-[calc(100vw-11rem)] truncate text-xs text-muted-foreground [font-variant-ligatures:no-contextual] sm:max-w-sm lg:max-w-md"
                title={item.original_name}
              >
                {item.original_name}
              </span>
              <span className="text-xs text-muted-foreground tabular-nums md:hidden">
                {typeLabel(item)} · {formatBytes(item.bytes)}
              </span>
            </div>
          );
        },
      }),
      col.display({
        id: "type",
        header: () => t("columns.type"),
        cell: (c) => <span className="whitespace-nowrap">{typeLabel(c.row.original)}</span>,
      }),
      col.display({
        id: "size",
        header: () => t("columns.size"),
        cell: (c) => (
          <span className="whitespace-nowrap tabular-nums">
            {formatBytes(c.row.original.bytes)}
          </span>
        ),
      }),
      col.display({
        id: "dimensions",
        header: () => t("columns.dimensions"),
        cell: (c) => {
          const dims = dimensions(c.row.original);
          return dims ? (
            <span className="whitespace-nowrap tabular-nums">{dims}</span>
          ) : (
            <span className="text-muted-foreground">
              <span aria-hidden="true">—</span>
              <span className="sr-only">{t("facts.unknown")}</span>
            </span>
          );
        },
      }),
      col.display({
        id: "author",
        header: () => t("columns.author"),
        cell: (c) => {
          const name = authorName(c.row.original);
          return (
            <span className="block max-w-36 truncate" title={name}>
              {name}
            </span>
          );
        },
      }),
      col.display({
        id: "uploaded",
        header: () => t("columns.uploaded"),
        cell: (c) => <RelativeTime at={c.row.original.created_at} className="whitespace-nowrap" />,
      }),
    ]);
  }, [t]);
}
