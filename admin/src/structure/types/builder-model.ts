// The field builder's model (07 §3 Content types). A type's schema is a list of
// `FieldDef`s (`{ "fields": [ … ] }` on the wire; `null`, no schema, is an empty builder);
// the builder edits rows that also carry a stable key, what the field was when last saved
// and whether its name still follows its label. `toSchema` sends options only for a
// `select`, sub-fields only for a `repeater` and target types only for a `relation` (and
// only when it lists any: empty is any type, P52), so a kind switched back and forth
// keeps them in the builder. `checkBuilder` mirrors core's schema rules
// (`crates/ferrite-core/src/fields.rs`: `check_level`, `check_options`, `valid_name`,
// `check_relation_types`) for immediate feedback; the server checks again, and
// `serverFieldError` puts its refusal of a relation's types at that field.

import {
  FEATURED_IMAGE,
  type FieldDef,
  type FieldKind,
  MAX_REPEATER_DEPTH,
} from "@/editor/fields/fields-model";
import { i18n } from "@/i18n";

/** Most fields per level (`fields.rs` `MAX_FIELDS`). */
export const MAX_FIELDS = 100;
/** Most options of a `select` (`MAX_OPTIONS`). */
export const MAX_OPTIONS = 100;
/** Longest field name (`MAX_NAME`). */
export const MAX_NAME = 64;
/** Longest label or option, in characters (`MAX_LABEL`). */
export const MAX_LABEL = 200;

/** Every kind, in the order the kind menu lists them. */
export const FIELD_KINDS: readonly FieldKind[] = [
  "text",
  "rich_text",
  "number",
  "date",
  "bool",
  "select",
  "relation",
  "media",
  "repeater",
];

export interface OptionRow {
  key: string;
  value: string;
}

export interface BuilderRow {
  key: string;
  name: string;
  label: string;
  kind: FieldKind;
  required: boolean;
  /** Kept while the kind is not `select`; sent only for one. */
  options: OptionRow[];
  /** Kept while the kind is not `repeater`; sent only for one. */
  fields: BuilderRow[];
  /** A relation's target types (empty: any type). Kept while the kind is not `relation`. */
  types: string[];
  /** The field's name and kind when last saved; `null` for a field added since. */
  saved: { name: string; kind: FieldKind } | null;
  /** The name follows the label (a new field whose name was never typed). */
  autoName: boolean;
}

let seq = 0;
/** A key no other row or option of this page has. */
export const newKey = (prefix = "f") => `${prefix}${++seq}`;

/** A new, empty text field whose name follows its label. */
export function newRow(): BuilderRow {
  return {
    key: newKey(),
    name: "",
    label: "",
    kind: "text",
    required: false,
    options: [],
    fields: [],
    types: [],
    saved: null,
    autoName: true,
  };
}

/** The builder's rows for a stored schema (`null`: no schema, no rows). */
export function fromSchema(value: readonly FieldDef[] | null | undefined): BuilderRow[] {
  return (value ?? []).map((d) => ({
    key: newKey(),
    name: d.name,
    label: d.label ?? "",
    kind: d.kind,
    required: d.required ?? false,
    options: (d.options ?? []).map((v) => ({ key: newKey("o"), value: v })),
    fields: fromSchema(d.fields),
    types: [...(d.types ?? [])],
    saved: { name: d.name, kind: d.kind },
    autoName: false,
  }));
}

/** The schema the rows describe, every member present (as the server answers). */
export function toSchema(rows: readonly BuilderRow[]): FieldDef[] {
  return rows.map((r) => ({
    name: r.name,
    label: r.label.trim(),
    kind: r.kind,
    required: r.required,
    options: r.kind === "select" ? r.options.map((o) => o.value) : [],
    fields: r.kind === "repeater" ? toSchema(r.fields) : [],
    ...(r.kind === "relation" && r.types.length > 0 ? { types: [...r.types] } : {}),
  }));
}

/** The rows as just saved: each remembers its name and kind, none follows its label. */
export function markSaved(rows: readonly BuilderRow[]): BuilderRow[] {
  return rows.map((r) => ({
    ...r,
    saved: { name: r.name, kind: r.kind },
    autoName: false,
    fields: markSaved(r.fields),
  }));
}

/**
 * A name for `label`: lower-case ASCII (accents dropped), every other character `_`,
 * repeats collapsed, cut to start with a letter, at most `max` long, no `_` at the end.
 * `""` when nothing is left (the user types one).
 */
export function nameFromLabel(label: string, max = MAX_NAME): string {
  const ascii = label
    .normalize("NFD")
    .replace(/\p{M}/gu, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "_")
    .replace(/^[^a-z]+/, "");
  return ascii.slice(0, max).replace(/_+$/, "");
}

/** `[a-z][a-z0-9_]{0,63}` (`fields.rs` `valid_name`). */
export const validName = (name: string) =>
  /^[a-z][a-z0-9_]*$/.test(name) && name.length <= MAX_NAME;

export type ErrorMember = "name" | "label" | "kind" | "options" | "option" | "fields" | "types";

/** What a relation cannot point at (`fields.rs`: media is picked by `media` fields). */
const MEDIA_TYPE = "media";

/**
 * Where an error of `checkBuilder` is: `<row key>:<member>`, an option's as
 * `<row key>:option:<option key>`; a level's own (too many fields) is the parent row's
 * `fields`, the top level's `:fields`.
 */
export function errorKey(rowKey: string | null, member: ErrorMember, optionKey?: string): string {
  return `${rowKey ?? ""}:${member}${optionKey === undefined ? "" : `:${optionKey}`}`;
}

const chars = (s: string) => [...s].length;
const msg = (key: string, values: Record<string, unknown> = {}) =>
  i18n.t(`structure:errors.${key}` as "structure:errors.name", values);

/**
 * Every problem of the rows, by `errorKey` (empty: core would take the schema).
 * `registered`: the content types' names, to check relation target types against (not
 * checked without it).
 */
export function checkBuilder(
  rows: readonly BuilderRow[],
  registered?: readonly string[],
): Record<string, string> {
  const out: Record<string, string> = {};
  const put = (key: string, message: string) => {
    if (!(key in out)) out[key] = message;
  };
  const level = (defs: readonly BuilderRow[], parent: string | null, depth: number) => {
    if (defs.length > MAX_FIELDS)
      put(errorKey(parent, "fields"), msg("tooMany", { max: MAX_FIELDS }));
    const seen = new Set<string>();
    for (const r of defs) {
      if (r.name === "") put(errorKey(r.key, "name"), msg("nameEmpty"));
      else if (!validName(r.name)) put(errorKey(r.key, "name"), msg("name", { max: MAX_NAME }));
      else if (seen.has(r.name)) put(errorKey(r.key, "name"), msg("duplicate"));
      else if (depth === 0 && r.name === FEATURED_IMAGE)
        put(errorKey(r.key, "name"), msg("reserved"));
      seen.add(r.name);
      if (chars(r.label.trim()) > MAX_LABEL)
        put(errorKey(r.key, "label"), msg("label", { max: MAX_LABEL }));
      if (r.kind === "select") options(r);
      if (r.kind === "relation") targets(r);
      if (r.kind === "repeater") {
        if (depth + 1 > MAX_REPEATER_DEPTH) {
          put(errorKey(r.key, "kind"), msg("depth", { max: MAX_REPEATER_DEPTH }));
        } else if (r.fields.length === 0) {
          put(errorKey(r.key, "fields"), msg("noSubFields"));
        } else {
          level(r.fields, r.key, depth + 1);
        }
      }
    }
  };
  const options = (r: BuilderRow) => {
    if (r.options.length === 0 || r.options.length > MAX_OPTIONS) {
      put(errorKey(r.key, "options"), msg("options", { max: MAX_OPTIONS }));
    }
    const seen = new Set<string>();
    for (const o of r.options) {
      const key = errorKey(r.key, "option", o.key);
      if (o.value === "") put(key, msg("optionEmpty"));
      else if (chars(o.value) > MAX_LABEL || o.value.includes("\0")) {
        put(key, msg("optionLong", { max: MAX_LABEL }));
      } else if (seen.has(o.value)) put(key, msg("optionDuplicate"));
      seen.add(o.value);
    }
  };
  const targets = (r: BuilderRow) => {
    const key = errorKey(r.key, "types");
    r.types.forEach((type, i) => {
      if (r.types.indexOf(type) < i) put(key, msg("typeTwice", { type }));
      else if (type === MEDIA_TYPE) put(key, msg("typeMedia"));
      else if (registered && !registered.includes(type)) put(key, msg("typeUnknown", { type }));
    });
  };
  level(rows, null, 0);
  return out;
}

/** The server's refusals that belong to a relation's target types (`fields.rs`). */
const TYPES_REFUSAL =
  /is not a content type|cannot point at media|is listed twice|only relation fields have types/;

/**
 * Where the server's `422` message about a relation's types goes (`fields.<path>: …`,
 * the path by field names, a repeater's sub-fields one more segment), as an `errorKey`
 * with the message after the path; `null` for any other message (the form shows it).
 */
export function serverFieldError(
  rows: readonly BuilderRow[],
  message: string,
): { key: string; message: string } | null {
  const m = /^fields((?:\.[a-z][a-z0-9_]*)+): (.+)$/s.exec(message);
  if (!m?.[1] || !m[2] || !TYPES_REFUSAL.test(m[2])) return null;
  let level: readonly BuilderRow[] = rows;
  let found: BuilderRow | undefined;
  for (const name of m[1].slice(1).split(".")) {
    found = level.find((r) => r.name === name);
    if (!found) return null;
    level = found.fields;
  }
  return found ? { key: errorKey(found.key, "types"), message: m[2] } : null;
}

/** Whether `key`'s row (or anything inside it) has an error in `errors`. */
export function rowHasError(row: BuilderRow, errors: Record<string, string>): boolean {
  const prefix = `${row.key}:`;
  if (Object.keys(errors).some((k) => k.startsWith(prefix))) return true;
  return row.kind === "repeater" && row.fields.some((f) => rowHasError(f, errors));
}

/** The keys of the rows from the top down to the first row with an error (to open them). */
export function pathToFirstError(
  rows: readonly BuilderRow[],
  errors: Record<string, string>,
): string[] {
  for (const r of rows) {
    if (!rowHasError(r, errors)) continue;
    const own = Object.keys(errors).some((k) => k.startsWith(`${r.key}:`));
    if (own || r.kind !== "repeater") return [r.key];
    return [r.key, ...pathToFirstError(r.fields, errors)];
  }
  return [];
}
