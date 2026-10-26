// A `repeater` field: its rows as cards of the sub-fields, reordered with the kit's
// `SortableList` (a drag handle that also works from the keyboard, and Move up / Move
// down in each row's menu), Add row and Remove row, at most 200 rows (core's limit).
// Repeaters nest as far as the schema allows (core: two levels); each row's inputs carry
// their path (`fields.chapters[1].title`), so errors land on the right input.

import { PlusIcon, Trash2Icon } from "lucide-react";
import { useCallback, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { cn } from "cn";
import { SortableList } from "@/components/kit/sortable-list";
import { Button } from "@/components/ui/button";
import { FieldInput, FieldShell } from "./field-input";
import { useFieldsContext } from "./fields-context";
import { type FieldDef, type FieldValues, MAX_ROWS, emptyRow, isObject } from "./fields-model";

interface Row {
  key: string;
  value: unknown;
  index: number;
}

let seq = 0;
const newKey = () => `row-${++seq}`;

export function RepeaterField({
  def,
  value,
  onChange,
  path,
  depth,
}: {
  def: FieldDef;
  value: unknown;
  onChange: (value: unknown) => void;
  path: string;
  depth: number;
}) {
  const { t } = useTranslation("editor");
  const { disabled, errorFor } = useFieldsContext();
  const subs = useMemo(() => def.fields ?? [], [def.fields]);
  const rows: unknown[] = Array.isArray(value) ? value : [];
  // Stable keys for the rows (the values have none), kept in step with their order.
  const [keys, setKeys] = useState<string[]>(() => rows.map(newKey));
  let current = keys;
  if (keys.length !== rows.length) {
    current =
      keys.length < rows.length
        ? [...keys, ...rows.slice(keys.length).map(newKey)]
        : keys.slice(0, rows.length);
    setKeys(current);
  }
  const items: Row[] = rows.map((v, i) => ({ key: current[i] ?? `row-x${i}`, value: v, index: i }));
  const label = def.label?.trim() || def.name;

  const setRow = (index: number, next: FieldValues) => {
    onChange(rows.map((r, i) => (i === index ? next : r)));
  };
  const remove = (index: number) => {
    setKeys(current.filter((_, i) => i !== index));
    onChange(rows.filter((_, i) => i !== index));
  };
  const add = () => {
    setKeys([...current, newKey()]);
    onChange([...rows, emptyRow(subs)]);
  };
  // A row's name: its position, and its first text value when it has one ("Alpha").
  const rowLabel = useCallback(
    (row: Row) => {
      const obj = isObject(row.value) ? row.value : {};
      const first = subs.find((f) => f.kind === "text" && typeof obj[f.name] === "string");
      const preview = first ? String(obj[first.name]).trim().slice(0, 60) : "";
      return preview
        ? t("fields.rowLabelNamed", { n: row.index + 1, label, preview })
        : t("fields.rowLabel", { n: row.index + 1, label });
    },
    [t, label, subs],
  );

  return (
    <FieldShell def={def} path={path} as="group">
      {() => (
        <div className="flex flex-col gap-2">
          {items.length === 0 ? (
            <p className="rounded-lg border border-dashed px-3 py-3 text-sm text-muted-foreground">
              {t("fields.noRows")}
            </p>
          ) : (
            <SortableList
              items={items}
              getId={(r) => r.key}
              label={rowLabel}
              name={t("fields.rowsLabel", { label })}
              onReorder={(next) => {
                setKeys(next.map((r) => r.key));
                onChange(next.map((r) => r.value));
              }}
              className="gap-2"
              render={(row) => {
                const rowPath = `${path}[${row.index}]`;
                const obj = isObject(row.value) ? row.value : {};
                return (
                  <div
                    data-slot="repeater-row"
                    className={cn("flex flex-col gap-3 py-2 pr-1", depth > 0 && "py-1.5")}
                  >
                    <div className="flex items-center justify-between gap-2">
                      <span className="text-xs font-medium text-muted-foreground tabular-nums">
                        {t("fields.row", { n: row.index + 1 })}
                      </span>
                      <Button
                        type="button"
                        variant="ghost"
                        size="icon-sm"
                        disabled={disabled}
                        aria-label={t("fields.removeRow", { row: rowLabel(row) })}
                        onClick={() => remove(row.index)}
                        className="text-muted-foreground hover:text-destructive pointer-coarse:size-11"
                      >
                        <Trash2Icon />
                      </Button>
                    </div>
                    <RowErrors
                      messages={[
                        errorFor(rowPath),
                        // Keys the row's schema does not have (a server message at them).
                        ...Object.keys(obj)
                          .filter((k) => !subs.some((f) => f.name === k))
                          .map((k) => errorFor(`${rowPath}.${k}`)),
                      ]}
                    />
                    {subs.map((sub) => (
                      <FieldInput
                        key={sub.name}
                        def={sub}
                        value={obj[sub.name]}
                        path={`${rowPath}.${sub.name}`}
                        depth={depth + 1}
                        onChange={(v) => setRow(row.index, { ...obj, [sub.name]: v })}
                      />
                    ))}
                  </div>
                );
              }}
            />
          )}
          <div className="flex flex-wrap items-center gap-3">
            <Button
              type="button"
              variant="outline"
              size="sm"
              disabled={disabled || rows.length >= MAX_ROWS}
              onClick={add}
              className="pointer-coarse:h-11"
            >
              <PlusIcon data-icon="inline-start" />
              {t("fields.addRow")}
            </Button>
            <span className="text-xs text-muted-foreground tabular-nums">
              {t("fields.rowCount", { count: rows.length, max: MAX_ROWS })}
            </span>
          </div>
        </div>
      )}
    </FieldShell>
  );
}

/** A row's own messages (not a row of fields, a key its schema lacks): shown in the row. */
function RowErrors({ messages }: { messages: (string | undefined)[] }) {
  const shown = messages.filter((m): m is string => m !== undefined);
  if (shown.length === 0) return null;
  return (
    <ul data-slot="row-errors" className="text-sm text-destructive">
      {shown.map((m) => (
        <li key={m}>{m}</li>
      ))}
    </ul>
  );
}
