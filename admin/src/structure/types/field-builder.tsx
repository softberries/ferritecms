// The field builder (07 §3 Content types): the fields of one level as a `SortableList`
// (drag handle that also works from the keyboard; Move up / Move down in each row's
// menu). Each row shows the label, the name in mono, the kind and whether it is required;
// opening a row shows its `FieldEditor` in place, which holds a nested builder for a
// repeater's sub-fields (core nests repeaters at most two deep).

import { CircleAlertIcon, ChevronDownIcon, PlusIcon } from "lucide-react";
import { cn } from "cn";
import { useCallback, useId } from "react";
import { useTranslation } from "react-i18next";
import { SortableList } from "@/components/kit/sortable-list";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { useBuilder } from "./builder-context";
import { type BuilderRow, MAX_FIELDS, errorKey, newRow, rowHasError } from "./builder-model";
import { FieldEditor } from "./field-editor";
import { KIND_ICONS } from "./kinds";

export interface FieldBuilderProps {
  rows: BuilderRow[];
  onChange: (rows: BuilderRow[]) => void;
  /** 0 at the top; a repeater's sub-fields are one deeper. */
  depth: number;
  /** The repeater whose sub-fields these are (`null` at the top). */
  parent: BuilderRow | null;
}

export function FieldBuilder({ rows, onChange, depth, parent }: FieldBuilderProps) {
  const { t } = useTranslation("structure");
  const b = useBuilder();
  const levelError = b.errors[errorKey(parent?.key ?? null, "fields")];
  const labelOf = useCallback(
    (r: BuilderRow) => r.label.trim() || r.name || t("builder.untitled"),
    [t],
  );
  const parentLabel = parent ? labelOf(parent) : null;
  const replace = (next: BuilderRow) => onChange(rows.map((r) => (r.key === next.key ? next : r)));
  const remove = (key: string) => onChange(rows.filter((r) => r.key !== key));
  const add = () => {
    const row = newRow();
    onChange([...rows, row]);
    b.setOpen(row.key, true);
    b.setFocusKey(row.key);
  };

  return (
    <div data-slot="field-builder" data-depth={depth} className="flex min-w-0 flex-col gap-2">
      {rows.length === 0 ? (
        <p className="rounded-lg border border-dashed px-3 py-3 text-sm text-muted-foreground">
          {depth === 0 ? t("builder.empty") : t("builder.emptyNested")}
        </p>
      ) : (
        <SortableList
          items={rows}
          getId={(r) => r.key}
          label={labelOf}
          name={parentLabel ? t("builder.list", { label: parentLabel }) : t("builder.listTop")}
          onReorder={onChange}
          rowClassName="items-start"
          render={(row) => (
            <FieldRow
              row={row}
              depth={depth}
              label={labelOf(row)}
              onChange={replace}
              onRemove={() => remove(row.key)}
            />
          )}
        />
      )}
      {levelError ? (
        <p data-slot="form-error" className="text-sm text-destructive">
          {levelError}
        </p>
      ) : null}
      <div className="flex flex-wrap items-center gap-3">
        <Button
          type="button"
          variant="outline"
          disabled={b.disabled || rows.length >= MAX_FIELDS}
          onClick={add}
        >
          <PlusIcon data-icon="inline-start" />
          {depth === 0 ? t("builder.add") : t("builder.addSub")}
        </Button>
        <span className="text-xs text-muted-foreground tabular-nums">
          {t("builder.count", { count: rows.length, max: MAX_FIELDS })}
        </span>
      </div>
    </div>
  );
}

function FieldRow({
  row,
  depth,
  label,
  onChange,
  onRemove,
}: {
  row: BuilderRow;
  depth: number;
  label: string;
  onChange: (row: BuilderRow) => void;
  onRemove: () => void;
}) {
  const { t } = useTranslation("structure");
  const b = useBuilder();
  const editorId = useId();
  const open = b.isOpen(row.key);
  const Icon = KIND_ICONS[row.kind];
  const invalid = rowHasError(row, b.errors);
  const kindChanged = row.saved !== null && row.saved.kind !== row.kind;
  return (
    <div
      data-slot="builder-row"
      data-name={row.name}
      data-open={open || undefined}
      className="flex min-w-0 flex-col"
    >
      <button
        type="button"
        aria-expanded={open}
        aria-controls={editorId}
        onClick={() => b.setOpen(row.key, !open)}
        className="flex min-h-control w-full min-w-0 items-center gap-3 rounded-md py-1 pr-1 pl-1 text-left transition-colors hover:bg-muted/50"
      >
        <span
          aria-hidden="true"
          className="flex size-8 shrink-0 items-center justify-center rounded-md border bg-muted/40 text-muted-foreground"
        >
          <Icon className="size-4" />
        </span>
        <span className="flex min-w-0 flex-1 flex-col gap-1">
          <span className="truncate text-sm font-medium text-foreground" title={label}>
            {label}
          </span>
          <span className="flex min-w-0 flex-wrap items-center gap-1.5">
            <code
              className="max-w-full truncate font-mono text-xs text-muted-foreground"
              title={row.name}
            >
              {row.name || "—"}
            </code>
            <Badge variant="outline" className="font-normal">
              {t(`kinds.${row.kind}`)}
            </Badge>
            {row.required ? <Badge variant="secondary">{t("builder.required")}</Badge> : null}
            {row.saved === null ? (
              <Badge variant="outline" className="border-primary/40 text-primary">
                {t("builder.new")}
              </Badge>
            ) : kindChanged ? (
              <Badge variant="outline" className="border-warning/50 text-warning">
                {t("builder.kindChanged")}
              </Badge>
            ) : null}
          </span>
        </span>
        {invalid ? (
          <span
            className="flex shrink-0 items-center text-destructive"
            title={t("builder.hasErrors")}
          >
            <CircleAlertIcon aria-hidden="true" className="size-4" />
            <span className="sr-only">{t("builder.hasErrors")}</span>
          </span>
        ) : null}
        <ChevronDownIcon
          aria-hidden="true"
          className={cn(
            "size-4 shrink-0 text-muted-foreground transition-transform duration-(--dur-fast)",
            open && "rotate-180",
          )}
        />
      </button>
      {open ? (
        // The row's handle and menu sit at its top; the open editor takes the whole row's
        // width below them (on a phone, nested sub-fields would be too narrow otherwise).
        <div
          id={editorId}
          className="mt-1 -mr-[calc(var(--control-h)+0.25rem)] -ml-[calc(var(--control-h)+0.25rem)] border-t px-2 pt-4 pb-2"
        >
          <FieldEditor
            row={row}
            depth={depth}
            label={label}
            onChange={onChange}
            onRemove={onRemove}
            onDone={() => b.setOpen(row.key, false)}
          />
        </div>
      ) : null}
    </div>
  );
}
