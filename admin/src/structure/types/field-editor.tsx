// One field of the builder, open (07 §3 Content types): label, name (following the label
// until typed, for a new field), kind (a saved field may change kind, with a warning that
// existing values may no longer be valid), required, the options of a `select` (a
// `SortableList` of strings, 1 to 100, no duplicates), the content types a `relation` may
// point at (checkboxes, none checked: any type; P52) and the sub-fields of a `repeater`
// (the builder again, one level deeper; core nests repeaters at most two deep).

import { useQuery } from "@tanstack/react-query";
import { CheckIcon, PlusIcon, Trash2Icon, TriangleAlertIcon } from "lucide-react";
import { useCallback, useEffect, useId, useMemo, useRef } from "react";
import { useTranslation } from "react-i18next";
import { SortableList } from "@/components/kit/sortable-list";
import { type ContentTypeDto, typesQuery } from "@/api/types";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { type FieldKind, MAX_REPEATER_DEPTH } from "@/editor/fields/fields-model";
import { useBuilder } from "./builder-context";
import {
  type BuilderRow,
  FIELD_KINDS,
  MAX_OPTIONS,
  type OptionRow,
  errorKey,
  nameFromLabel,
  newKey,
  validName,
} from "./builder-model";
import { FieldBuilder } from "./field-builder";
import { FormField, SwitchRow } from "@/components/kit/form-field";
import { KIND_ICONS } from "./kinds";

export interface FieldEditorProps {
  row: BuilderRow;
  depth: number;
  /** The row's display name (label, else name). */
  label: string;
  onChange: (row: BuilderRow) => void;
  onRemove: () => void;
  onDone: () => void;
}

export function FieldEditor({ row, depth, label, onChange, onRemove, onDone }: FieldEditorProps) {
  const { t } = useTranslation("structure");
  const b = useBuilder();
  const err = (member: "name" | "label" | "kind" | "options" | "fields") =>
    b.errors[errorKey(row.key, member)];
  const labelInput = useRef<HTMLInputElement>(null);
  const { focusKey, setFocusKey } = b;
  useEffect(() => {
    if (focusKey !== row.key) return;
    labelInput.current?.focus();
    setFocusKey(null);
  }, [focusKey, row.key, setFocusKey]);

  const set = (patch: Partial<BuilderRow>) => onChange({ ...row, ...patch });
  const kindWarning = row.saved !== null && row.saved.kind !== row.kind;
  const renamed =
    row.saved !== null && row.saved.name !== row.name && validName(row.name)
      ? row.saved.name
      : null;
  // A repeater at the deepest level could hold no repeater; one already there says so.
  const repeaterAllowed = depth + 1 <= MAX_REPEATER_DEPTH;

  return (
    <div data-slot="field-editor" className="flex min-w-0 flex-col gap-5">
      <div className="grid gap-4 sm:grid-cols-2">
        <FormField
          label={t("editor.label")}
          hint={t("editor.labelHint")}
          error={err("label")}
          slot="field-label"
        >
          {(control) => (
            <Input
              {...control}
              ref={labelInput}
              value={row.label}
              disabled={b.disabled}
              autoComplete="off"
              onChange={(e) => {
                const value = e.currentTarget.value;
                set(row.autoName ? { label: value, name: nameFromLabel(value) } : { label: value });
              }}
            />
          )}
        </FormField>
        <FormField
          label={t("editor.name")}
          hint={renamed ? t("editor.renameNote", { name: renamed }) : t("editor.nameHint")}
          error={err("name")}
          slot="field-name"
        >
          {(control) => (
            <Input
              {...control}
              value={row.name}
              disabled={b.disabled}
              autoComplete="off"
              spellCheck={false}
              className="font-mono"
              onChange={(e) => set({ name: e.currentTarget.value, autoName: false })}
            />
          )}
        </FormField>
      </div>

      <div className="grid gap-4 sm:grid-cols-2">
        <FormField
          label={t("editor.kind")}
          error={err("kind")}
          hint={
            repeaterAllowed
              ? t(`kindHints.${row.kind}`)
              : `${t(`kindHints.${row.kind}`)}. ${t("editor.kindDepth", { max: MAX_REPEATER_DEPTH })}`
          }
          slot="field-kind"
        >
          {(control) => (
            <Select
              value={row.kind}
              disabled={b.disabled}
              onValueChange={(v) => set({ kind: v as FieldKind })}
            >
              <SelectTrigger {...control} className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent position="popper" className="max-h-80">
                {FIELD_KINDS.map((kind) => {
                  const Icon = KIND_ICONS[kind];
                  return (
                    <SelectItem
                      key={kind}
                      value={kind}
                      disabled={kind === "repeater" && !repeaterAllowed && row.kind !== kind}
                    >
                      <Icon className="text-muted-foreground" />
                      {t(`kinds.${kind}`)}
                    </SelectItem>
                  );
                })}
              </SelectContent>
            </Select>
          )}
        </FormField>
        <div className="flex flex-col sm:pt-6">
          <SwitchRow label={t("editor.required")} hint={t("editor.requiredHint")}>
            {(control) => (
              <Switch
                {...control}
                checked={row.required}
                disabled={b.disabled}
                onCheckedChange={(required) => set({ required })}
              />
            )}
          </SwitchRow>
        </div>
      </div>

      {kindWarning && row.saved ? (
        <p
          data-slot="kind-warning"
          className="flex items-start gap-2 rounded-lg border border-warning/40 bg-warning/8 px-3 py-2 text-sm text-foreground"
        >
          <TriangleAlertIcon aria-hidden="true" className="mt-0.5 size-4 shrink-0 text-warning" />
          {t("editor.kindWarning", { kind: t(`kinds.${row.saved.kind}`) })}
        </p>
      ) : null}

      {row.kind === "select" ? (
        <OptionsEditor
          row={row}
          label={label}
          onChange={(options) => set({ options })}
          error={err("options")}
        />
      ) : null}

      {row.kind === "relation" ? (
        <TargetsEditor
          types={row.types}
          onChange={(types) => set({ types })}
          error={b.errors[errorKey(row.key, "types")]}
        />
      ) : null}

      {row.kind === "repeater" && repeaterAllowed ? (
        <section
          aria-label={t("editor.subFields")}
          className="flex min-w-0 flex-col gap-2 rounded-lg border bg-muted/30 p-3"
        >
          <div className="flex flex-col gap-0.5">
            <h4 className="text-sm font-medium">{t("editor.subFields")}</h4>
            <p className="text-xs text-muted-foreground">{t("editor.subFieldsHint", { label })}</p>
          </div>
          <FieldBuilder
            rows={row.fields}
            onChange={(fields) => set({ fields })}
            depth={depth + 1}
            parent={row}
          />
          {err("fields") ? (
            <p data-slot="form-error" className="text-sm text-destructive">
              {err("fields")}
            </p>
          ) : null}
        </section>
      ) : null}

      <div className="flex flex-wrap items-center justify-between gap-2 border-t pt-3">
        <Button
          type="button"
          variant="ghost"
          disabled={b.disabled}
          onClick={onRemove}
          className="text-destructive hover:bg-destructive/10 hover:text-destructive"
        >
          <Trash2Icon data-icon="inline-start" />
          {t("editor.remove")}
        </Button>
        <Button type="button" variant="outline" onClick={onDone}>
          <CheckIcon data-icon="inline-start" />
          {t("editor.done")}
        </Button>
      </div>
    </div>
  );
}

function OptionsEditor({
  row,
  label,
  onChange,
  error,
}: {
  row: BuilderRow;
  label: string;
  onChange: (options: OptionRow[]) => void;
  error: string | undefined;
}) {
  const { t } = useTranslation("structure");
  const b = useBuilder();
  const list = useRef<HTMLDivElement>(null);
  const options = row.options;
  const position = (o: OptionRow) => options.indexOf(o) + 1;
  const optionLabel = useCallback(
    (o: OptionRow) =>
      t("editor.optionLabel", {
        n: options.indexOf(o) + 1,
        value: o.value || t("editor.optionEmpty"),
      }),
    [options, t],
  );
  const add = () => {
    const next = [...options, { key: newKey("o"), value: "" }];
    onChange(next);
    // The new option's input takes the cursor once it exists.
    requestAnimationFrame(() => {
      const inputs = list.current?.querySelectorAll<HTMLInputElement>("[data-slot=option-input]");
      inputs?.[inputs.length - 1]?.focus();
    });
  };
  return (
    <section
      ref={list}
      data-slot="options-editor"
      aria-label={t("editor.options")}
      className="flex min-w-0 flex-col gap-2 rounded-lg border bg-muted/30 p-3"
    >
      <div className="flex flex-col gap-0.5">
        <h4 className="text-sm font-medium">{t("editor.options")}</h4>
        <p className="text-xs text-muted-foreground">
          {t("editor.optionsHint", { max: MAX_OPTIONS })}
        </p>
      </div>
      {options.length > 0 ? (
        <SortableList
          items={options}
          getId={(o) => o.key}
          label={optionLabel}
          name={t("editor.optionsList", { label })}
          onReorder={onChange}
          rowClassName="items-start"
          render={(o) => {
            const n = position(o);
            const message = b.errors[errorKey(row.key, "option", o.key)];
            const errorId = `${row.key}-${o.key}-error`;
            return (
              <div className="flex min-w-0 flex-col gap-1 py-px">
                <div className="flex items-center gap-1">
                  <Input
                    data-slot="option-input"
                    aria-label={t("editor.option", { n })}
                    value={o.value}
                    disabled={b.disabled}
                    autoComplete="off"
                    {...(message ? { "aria-invalid": true, "aria-describedby": errorId } : {})}
                    onChange={(e) => {
                      const value = e.currentTarget.value;
                      onChange(options.map((x) => (x.key === o.key ? { ...x, value } : x)));
                    }}
                    onKeyDown={(e) => {
                      if (e.key === "Enter" && !e.nativeEvent.isComposing) {
                        e.preventDefault();
                        if (options.length < MAX_OPTIONS) add();
                      }
                    }}
                  />
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    disabled={b.disabled}
                    aria-label={t("editor.removeOption", { n })}
                    className="text-muted-foreground hover:text-destructive"
                    onClick={() => onChange(options.filter((x) => x.key !== o.key))}
                  >
                    <Trash2Icon />
                  </Button>
                </div>
                {message ? (
                  <p id={errorId} data-slot="form-error" className="text-sm text-destructive">
                    {message}
                  </p>
                ) : null}
              </div>
            );
          }}
        />
      ) : null}
      {error ? (
        <p data-slot="form-error" className="text-sm text-destructive">
          {error}
        </p>
      ) : null}
      <div className="flex flex-wrap items-center gap-3">
        <Button
          type="button"
          variant="outline"
          disabled={b.disabled || options.length >= MAX_OPTIONS}
          onClick={add}
        >
          <PlusIcon data-icon="inline-start" />
          {t("editor.addOption")}
        </Button>
        <span className="text-xs text-muted-foreground tabular-nums">
          {t("builder.count", { count: options.length, max: MAX_OPTIONS })}
        </span>
      </div>
    </section>
  );
}

const BUILT_IN_ORDER = ["post", "page"];

/** The types a relation can point at: not media (`fields.rs`); built-ins first, then by label. */
function relationTargets(types: readonly ContentTypeDto[]): ContentTypeDto[] {
  const rank = (ty: ContentTypeDto) => {
    const i = BUILT_IN_ORDER.indexOf(ty.name);
    return i >= 0 ? i : ty.built_in ? BUILT_IN_ORDER.length : BUILT_IN_ORDER.length + 1;
  };
  return types
    .filter((ty) => ty.name !== "media")
    .sort((a, b) => rank(a) - rank(b) || a.label.localeCompare(b.label));
}

/**
 * A relation's target types as checkboxes (none checked: any type). A stored name that is
 * not offered (the type is gone) stays listed, checked, so it can be unchecked; the
 * builder's check says why it cannot be saved.
 */
function TargetsEditor({
  types,
  onChange,
  error,
}: {
  types: string[];
  onChange: (types: string[]) => void;
  error: string | undefined;
}) {
  const { t } = useTranslation("structure");
  const b = useBuilder();
  const id = useId();
  const all = useQuery(typesQuery);
  const offered = useMemo(() => relationTargets(all.data ?? []), [all.data]);
  // Until the types are loaded nothing is known to be gone (no false alarm).
  const unknown = all.data ? types.filter((name) => !offered.some((ty) => ty.name === name)) : [];
  const toggle = (name: string, on: boolean) =>
    onChange(on ? [...types.filter((v) => v !== name), name] : types.filter((v) => v !== name));
  const entries = [
    ...offered.map((ty) => ({ name: ty.name, label: ty.label })),
    ...unknown.map((name) => ({ name, label: null })),
  ];
  return (
    <fieldset
      data-slot="relation-targets"
      aria-describedby={`${id}-hint${error ? ` ${id}-error` : ""}`}
      className="flex min-w-0 flex-col gap-2 rounded-lg border bg-muted/30 p-3"
    >
      <legend className="float-left mb-0.5 text-sm font-medium">{t("editor.targets")}</legend>
      <p id={`${id}-hint`} className="clear-left text-xs text-muted-foreground">
        {t("editor.targetsHint")}
      </p>
      <ul className="grid gap-x-4 gap-y-1 sm:grid-cols-2">
        {entries.map((ty) => {
          const boxId = `${id}-${ty.name}`;
          return (
            <li key={ty.name} className="flex min-h-11 items-center gap-2.5 sm:min-h-9">
              <Checkbox
                id={boxId}
                checked={types.includes(ty.name)}
                disabled={b.disabled}
                // The save puts the cursor on the first input that says why.
                {...(error ? { "aria-invalid": true } : {})}
                onCheckedChange={(on) => toggle(ty.name, on === true)}
              />
              <Label htmlFor={boxId} className="min-w-0 font-normal">
                {ty.label === null ? (
                  <span className="truncate text-destructive" title={ty.name}>
                    {t("editor.targetsUnknown")}
                  </span>
                ) : (
                  <span className="truncate" title={ty.label}>
                    {ty.label}
                  </span>
                )}
                <code className="truncate font-mono text-xs text-muted-foreground" title={ty.name}>
                  {ty.name}
                </code>
              </Label>
            </li>
          );
        })}
      </ul>
      {error ? (
        <p id={`${id}-error`} data-slot="form-error" className="text-sm text-destructive">
          {error}
        </p>
      ) : null}
    </fieldset>
  );
}
