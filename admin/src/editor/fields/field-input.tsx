// One custom field's input (07 §3 Editor), by kind: `text` an auto-growing text area
// (the schema does not say single- or multi-line), `rich_text` a compact editor with the
// body's schema, `number` a decimal text field stored as a JSON number, `date` a calendar
// with a 24 h time stored as RFC 3339 UTC, `bool` a switch, `select` the field's options
// (with an empty choice when optional), `relation` a search over the content types, `media`
// the media picker, `repeater` rows of sub-fields. Each has its label, the field's name in
// mono as a hint, and its error (the check here, or the server's `422` at the same path).
// A cleared input stores `null` (core accepts it for an optional field; a required one
// says so).

import { useId, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
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
import { Textarea } from "@/components/ui/textarea";
import { DateField } from "./date-field";
import { type ControlIds, useFieldsContext } from "./fields-context";
import { parseNumber } from "@/lib/values";
import type { FieldDef } from "./fields-model";
import { MediaField } from "./media-field";
import { RelationField } from "./relation-field";
import { RepeaterField } from "./repeater-field";
import { RichTextField } from "./rich-text-field";

export interface FieldInputProps {
  def: FieldDef;
  value: unknown;
  onChange: (value: unknown) => void;
  /** The value's path (`fields.price`, `fields.chapters[0].title`). */
  path: string;
  /** How deep in repeaters (0 at the top). */
  depth?: number;
}

/** The label, the name hint, the control and the error of one field. */
export function FieldShell({
  def,
  path,
  children,
  as = "div",
}: {
  def: FieldDef;
  path: string;
  children: (ids: ControlIds) => ReactNode;
  /** `group`: a repeater (its label names a group, not one control). */
  as?: "div" | "group";
}) {
  const { t } = useTranslation("editor");
  const { errorFor } = useFieldsContext();
  const id = useId();
  const hintId = `${id}-hint`;
  const errorId = `${id}-error`;
  const labelId = `${id}-label`;
  const error = errorFor(path);
  const label = def.label?.trim() || def.name;
  const describedBy = error ? `${hintId} ${errorId}` : hintId;
  // A long label breaks anywhere rather than pushing the row past a phone's width.
  const head = (
    <div className="flex min-w-0 items-baseline justify-between gap-3">
      {as === "group" ? (
        <span id={labelId} className="min-w-0 text-sm leading-none font-medium wrap-anywhere">
          {label}
          {def.required ? <Required /> : null}
        </span>
      ) : (
        <Label htmlFor={id} id={labelId} className="min-w-0 wrap-anywhere">
          {label}
          {def.required ? <Required /> : null}
        </Label>
      )}
      <code
        id={hintId}
        className="min-w-0 truncate font-mono text-xs text-muted-foreground"
        title={def.name}
      >
        <span className="sr-only">{t("fields.nameHint")} </span>
        {def.name}
      </code>
    </div>
  );
  const body = (
    <>
      {head}
      {children({ id, labelId, describedBy, invalid: error !== undefined })}
      {error ? (
        <p id={errorId} data-slot="field-error" className="text-sm text-destructive">
          {error}
        </p>
      ) : null}
    </>
  );
  return as === "group" ? (
    <div
      role="group"
      aria-labelledby={labelId}
      aria-describedby={describedBy}
      data-slot="field"
      data-path={path}
      data-kind={def.kind}
      className="flex min-w-0 flex-col gap-2"
    >
      {body}
    </div>
  ) : (
    <div
      data-slot="field"
      data-path={path}
      data-kind={def.kind}
      className="flex min-w-0 flex-col gap-1.5"
    >
      {body}
    </div>
  );
}

function Required() {
  const { t } = useTranslation("editor");
  return (
    <>
      <span aria-hidden="true" className="ml-0.5 text-destructive">
        *
      </span>
      <span className="sr-only"> {t("fields.required")}</span>
    </>
  );
}

export function FieldInput({ def, value, onChange, path, depth = 0 }: FieldInputProps) {
  const { edited } = useFieldsContext();
  const change = (next: unknown) => {
    edited(path);
    onChange(next);
  };
  if (def.kind === "repeater") {
    return <RepeaterField def={def} value={value} onChange={change} path={path} depth={depth} />;
  }
  return (
    <FieldShell def={def} path={path}>
      {(ids) => <Control def={def} value={value} onChange={change} ids={ids} />}
    </FieldShell>
  );
}

function Control({
  def,
  value,
  onChange,
  ids,
}: {
  def: FieldDef;
  value: unknown;
  onChange: (value: unknown) => void;
  ids: ControlIds;
}) {
  const { disabled } = useFieldsContext();
  const common = {
    id: ids.id,
    "aria-describedby": ids.describedBy,
    ...(ids.invalid ? { "aria-invalid": true } : {}),
  };
  switch (def.kind) {
    case "text":
      return (
        <Textarea
          {...common}
          rows={1}
          disabled={disabled}
          value={
            typeof value === "string"
              ? value
              : typeof value === "number" || typeof value === "boolean"
                ? String(value)
                : ""
          }
          onChange={(e) => onChange(e.target.value === "" ? null : e.target.value)}
          className="min-h-9 resize-none"
        />
      );
    case "number":
      return <NumberInput common={common} value={value} onChange={onChange} disabled={disabled} />;
    case "bool":
      return (
        <div className="flex min-h-9 items-center">
          <Switch
            {...common}
            disabled={disabled}
            checked={value === true}
            onCheckedChange={(on) => onChange(on)}
          />
        </div>
      );
    case "select":
      return (
        <SelectInput
          def={def}
          common={common}
          value={value}
          onChange={onChange}
          disabled={disabled}
        />
      );
    case "date":
      return <DateField ids={ids} value={value} onChange={onChange} disabled={disabled} />;
    case "rich_text":
      return <RichTextField ids={ids} value={value} onChange={onChange} disabled={disabled} />;
    case "relation":
      return (
        <RelationField
          ids={ids}
          value={value}
          onChange={onChange}
          disabled={disabled}
          targets={def.types}
        />
      );
    case "media":
      return <MediaField ids={ids} value={value} onChange={onChange} disabled={disabled} />;
    case "repeater":
      return null;
  }
}

type Common = { id: string; "aria-describedby": string; "aria-invalid"?: boolean };

function NumberInput({
  common,
  value,
  onChange,
  disabled,
}: {
  common: Common;
  value: unknown;
  onChange: (v: unknown) => void;
  disabled: boolean;
}) {
  const shown = (v: unknown) =>
    typeof v === "number" ? String(v) : typeof v === "string" ? v : "";
  // The text as typed; a stored value that changes elsewhere (a restore) replaces it.
  const [text, setText] = useState(() => shown(value));
  const [seen, setSeen] = useState(value);
  if (!Object.is(seen, value)) {
    setSeen(value);
    if (!Object.is(parseNumber(text), value)) setText(shown(value));
  }
  return (
    <Input
      {...common}
      inputMode="decimal"
      autoComplete="off"
      disabled={disabled}
      value={text}
      onChange={(e) => {
        setText(e.target.value);
        const next = parseNumber(e.target.value);
        setSeen(next);
        onChange(next);
      }}
      className="w-full max-w-48 font-mono tabular-nums"
    />
  );
}

const NONE = "__none__";

function SelectInput({
  def,
  common,
  value,
  onChange,
  disabled,
}: {
  def: FieldDef;
  common: Common;
  value: unknown;
  onChange: (v: unknown) => void;
  disabled: boolean;
}) {
  const { t } = useTranslation("editor");
  const options = def.options ?? [];
  const current = typeof value === "string" ? value : "";
  // A stored value the options no longer have is still shown (and refused on save).
  const extra = current && !options.includes(current) ? [current] : [];
  return (
    <Select
      value={current === "" ? (def.required ? "" : NONE) : current}
      disabled={disabled}
      onValueChange={(v) => onChange(v === NONE ? null : v)}
    >
      <SelectTrigger {...common} className="w-full">
        <SelectValue placeholder={t("fields.selectPlaceholder")} />
      </SelectTrigger>
      <SelectContent>
        {def.required ? null : <SelectItem value={NONE}>{t("fields.selectNone")}</SelectItem>}
        {[...extra, ...options].map((o) => (
          <SelectItem key={o} value={o}>
            {o}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}
