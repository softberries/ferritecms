// Lists (11 §6): TanStack Table v9 with row selection, `j`/`k` to move, `x` to select,
// `enter` to open; actions that appear on row hover or focus (always shown on touch
// screens). The frame is its own scroll container on both axes, so a wide table never
// spills out of it, and the header sticks to the frame's top. The frame is at most the
// viewport below the shell's top bar (`--sticky-top`, 11 §7) tall. With `animateRows`,
// rows enter and leave with `AnimatePresence` and move with the layout spring (11 §5);
// under reduced motion they appear and go at once.

import { useTable, type Row, type RowData, type Updater } from "@tanstack/react-table";
import { cn } from "cn";
import { AnimatePresence, useReducedMotion } from "motion/react";
import * as m from "motion/react-m";
import { useCallback, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { Checkbox } from "@/components/ui/checkbox";
import { Skeleton } from "@/components/ui/skeleton";
import { TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { MOTION } from "@/design/tokens";
import {
  dataTableFeatures,
  type DataTableColumn,
  type DataTableFeatures,
  type RowSelectionState,
} from "./data-table-features";

export interface DataTableSelection {
  state: RowSelectionState;
  onChange: (next: RowSelectionState) => void;
}

export interface DataTableProps<T extends RowData> {
  columns: DataTableColumn<T>[];
  data: T[];
  getRowId: (row: T) => string;
  /** Adds the checkbox column and `x`. */
  selection?: DataTableSelection;
  /** `enter` on a row, or a click on it outside its controls. */
  onRowOpen?: (row: T) => void;
  loading: boolean;
  /** Shown in place of the rows when `data` is empty (usually an `EmptyState`). */
  empty: ReactNode;
  /** Actions shown at the end of a row on hover or focus. */
  rowActions?: (row: T) => ReactNode;
  /** Extra classes per column id, for header and body cells (e.g. `hidden md:table-cell`). */
  columnClassName?: Record<string, string>;
  /** The table's accessible name. */
  label: string;
  /** Rows enter, leave and move with motion (11 §5); none under reduced motion. */
  animateRows?: boolean;
  /** Rendered inside the scrolling frame, below the table (a "Load more" row, a sentinel). */
  footer?: ReactNode;
  className?: string;
}

/** 11 §5: the layout spring (list insert, remove, reorder). */
const LAYOUT_SPRING = { type: "spring", stiffness: 520, damping: 42, mass: 1 } as const;
const SELECT = "__select";
const ACTIONS = "__actions";
const SKELETON_ROWS = 5;
const INTERACTIVE = "a, button, input, textarea, select, [role=checkbox], [role=menuitem]";
// `TableRow`'s classes, for the `m.tr`, except the selected tint: a 4 % ember wash, not
// `muted`, on which the warning-coloured "Pending review" badge drops below 4.5:1.
const ROW =
  "border-b transition-colors hover:bg-muted/50 has-aria-expanded:bg-muted/50 data-[state=selected]:bg-primary/4";

export function DataTable<T extends RowData>({
  columns,
  data,
  getRowId,
  selection,
  onRowOpen,
  loading,
  empty,
  rowActions,
  columnClassName,
  label,
  animateRows = false,
  footer,
  className,
}: DataTableProps<T>) {
  const { t } = useTranslation();
  const reduce = useReducedMotion() ?? false;
  const motionOn = animateRows && !reduce;
  const [active, setActive] = useState(0);
  const body = useRef<HTMLTableSectionElement>(null);

  const allColumns = useMemo(() => {
    const out: DataTableColumn<T>[] = [];
    if (selection) {
      out.push({
        id: SELECT,
        header: ({ table }) => (
          <Checkbox
            aria-label={t("kit.table.selectAll")}
            checked={
              table.getIsAllRowsSelected()
                ? true
                : table.getIsSomeRowsSelected()
                  ? "indeterminate"
                  : false
            }
            onCheckedChange={(v) => table.toggleAllRowsSelected(v === true)}
          />
        ),
        cell: ({ row }) => (
          <Checkbox
            aria-label={t("kit.table.selectRow")}
            checked={row.getIsSelected()}
            onCheckedChange={(v) => row.toggleSelected(v === true)}
          />
        ),
      });
    }
    out.push(...columns);
    if (rowActions) {
      out.push({
        id: ACTIONS,
        header: () => <span className="sr-only">{t("kit.table.actions")}</span>,
        cell: ({ row }) => (
          <div className="flex justify-end gap-1 opacity-0 transition-opacity group-hover/row:opacity-100 group-focus-within/row:opacity-100 group-data-[state=selected]/row:opacity-100 has-aria-expanded:opacity-100 pointer-coarse:opacity-100">
            {rowActions(row.original)}
          </div>
        ),
      });
    }
    return out;
  }, [columns, rowActions, selection, t]);

  const selected = selection?.state;
  const onSelectionChange = selection?.onChange;
  const onRowSelectionChange = useCallback(
    (updater: Updater<RowSelectionState>) => {
      if (!onSelectionChange) return;
      const current = selected ?? {};
      onSelectionChange(typeof updater === "function" ? updater(current) : updater);
    },
    [onSelectionChange, selected],
  );

  const table = useTable({
    features: dataTableFeatures,
    columns: allColumns,
    data,
    getRowId: (row: T) => getRowId(row),
    enableRowSelection: selection !== undefined,
    state: { rowSelection: selected ?? {} },
    onRowSelectionChange,
  });
  const rows = table.getRowModel().rows;
  const current = Math.min(active, Math.max(rows.length - 1, 0));

  const focusRow = (index: number) => {
    const next = Math.max(0, Math.min(rows.length - 1, index));
    setActive(next);
    body.current?.querySelectorAll<HTMLTableRowElement>("tr[data-row]")[next]?.focus();
  };

  const onRowKeyDown = (
    event: KeyboardEvent<HTMLTableRowElement>,
    row: Row<DataTableFeatures, T>,
    index: number,
  ) => {
    if (event.target !== event.currentTarget) return;
    if (event.metaKey || event.ctrlKey || event.altKey) return;
    switch (event.key) {
      case "j":
      case "ArrowDown":
        event.preventDefault();
        focusRow(index + 1);
        break;
      case "k":
      case "ArrowUp":
        event.preventDefault();
        focusRow(index - 1);
        break;
      case "x":
        if (!selection) return;
        event.preventDefault();
        row.toggleSelected();
        break;
      case "Enter":
        if (!onRowOpen) return;
        event.preventDefault();
        onRowOpen(row.original);
        break;
    }
  };

  const headerGroups = table.getHeaderGroups();
  const leafIds = table.getAllLeafColumns().map((c) => c.id);
  const colCount = allColumns.length;
  const cellClass = (id: string) =>
    cn(id === SELECT && "w-10 pr-0", id === ACTIONS && "w-px text-right", columnClassName?.[id]);

  return (
    <m.div
      data-slot="data-table"
      layoutScroll={motionOn}
      className={cn(
        "relative max-h-[calc(100svh-var(--sticky-top)-var(--page-pad))] w-full overflow-auto rounded-xl border bg-card",
        className,
      )}
    >
      <table
        aria-label={label}
        aria-busy={loading || undefined}
        className="w-full caption-bottom border-separate border-spacing-0 text-base"
      >
        <TableHeader className="sticky top-0 z-10 bg-card [&_tr]:border-b-0">
          {headerGroups.map((group) => (
            <TableRow key={group.id} className="hover:bg-transparent">
              {group.headers.map((header) => (
                <TableHead
                  key={header.id}
                  className={cn(
                    "h-10 border-b px-3 text-sm font-medium text-muted-foreground first:rounded-tl-xl first:pl-4 last:rounded-tr-xl last:pr-4",
                    cellClass(header.column.id),
                  )}
                >
                  {header.isPlaceholder ? null : <table.FlexRender header={header} />}
                </TableHead>
              ))}
            </TableRow>
          ))}
        </TableHeader>
        <TableBody ref={body}>
          {loading ? (
            Array.from({ length: SKELETON_ROWS }, (_, i) => (
              <TableRow key={`skeleton-${i}`} className="group/row h-row hover:bg-transparent">
                {leafIds.map((id, j) => (
                  <TableCell
                    key={id}
                    className={cn(
                      "border-b px-3 py-0 group-last/row:border-b-0 first:pl-4 last:pr-4",
                      cellClass(id),
                    )}
                  >
                    <Skeleton
                      className={cn(
                        "h-3.5",
                        id === SELECT ? "size-4 rounded-sm" : j <= 1 ? "w-40 max-w-full" : "w-16",
                      )}
                    />
                  </TableCell>
                ))}
              </TableRow>
            ))
          ) : rows.length === 0 ? (
            <TableRow className="hover:bg-transparent">
              <TableCell colSpan={colCount} className="p-4 whitespace-normal">
                {empty}
              </TableCell>
            </TableRow>
          ) : (
            <AnimatePresence initial={false}>
              {rows.map((row, index) => (
                <m.tr
                  key={row.id}
                  data-slot="table-row"
                  data-row=""
                  data-state={row.getIsSelected() ? "selected" : undefined}
                  tabIndex={index === current ? 0 : -1}
                  onFocus={(e) => {
                    if (e.target === e.currentTarget) setActive(index);
                  }}
                  onKeyDown={(e) => onRowKeyDown(e, row, index)}
                  onClick={(e) => {
                    if (!onRowOpen) return;
                    if ((e.target as HTMLElement).closest(INTERACTIVE)) return;
                    onRowOpen(row.original);
                  }}
                  className={cn(
                    ROW,
                    "group/row h-row -outline-offset-2",
                    onRowOpen && "cursor-pointer",
                  )}
                  {...(motionOn
                    ? {
                        layout: "position" as const,
                        initial: { opacity: 0 },
                        animate: { opacity: 1 },
                        exit: { opacity: 0 },
                        transition: {
                          opacity: { duration: MOTION.durBase, ease: MOTION.easeOut },
                          layout: LAYOUT_SPRING,
                        },
                      }
                    : {})}
                >
                  {row.getAllCells().map((cell) => (
                    <TableCell
                      key={cell.id}
                      className={cn(
                        "border-b px-3 py-0 group-last/row:border-b-0 first:pl-4 last:pr-4",
                        cellClass(cell.column.id),
                      )}
                    >
                      <table.FlexRender cell={cell} />
                    </TableCell>
                  ))}
                </m.tr>
              ))}
            </AnimatePresence>
          )}
        </TableBody>
      </table>
      {footer}
      {loading ? <span className="sr-only">{t("kit.table.loading")}</span> : null}
    </m.div>
  );
}
