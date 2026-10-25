// The TanStack Table v9 features every `DataTable` uses; columns are typed against it.

import {
  createColumnHelper,
  rowSelectionFeature,
  tableFeatures,
  type ColumnHelper,
  type RowData,
  type RowSelectionState,
} from "@tanstack/react-table";

export const dataTableFeatures = tableFeatures({ rowSelectionFeature });
export type DataTableFeatures = typeof dataTableFeatures;

/**
 * One column of any value type: the element type of `columnHelper.columns([...])`, so
 * accessor columns of different value types fit in one array (as the library types it).
 */
export type DataTableColumn<T extends RowData> = ReturnType<
  ColumnHelper<DataTableFeatures, T>["columns"]
>[number];
export type { RowSelectionState };

/** Column helper bound to the DataTable features. */
export function dataTableColumns<T extends RowData>() {
  return createColumnHelper<DataTableFeatures, T>();
}
