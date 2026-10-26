// Custom fields as the editor sees them (07 §3; core rules in
// `crates/ferrite-core/src/fields.rs`, mirrored here for immediate feedback). A type with
// no schema (`fields: null`) shows no fields section and its items' `fields` object is
// kept as stored, `featured_image` aside (P36). With a schema, keys it no longer has are
// listed and dropped on the next save (P39), since core refuses unknown keys on every
// write; everything else is sent as edited, `null` included (a cleared optional field).
//
// Paths are core's: `fields.price`, `fields.chapters[2].title`. The server's `422`
// (`errors[].field` = `fields`, the path at the start of `detail`) maps onto the same
// inputs (`serverFieldErrors`).

import { ApiError } from "@/api/problem";
import type { SchemaFieldDefDto } from "@/api/schema";
import type { ContentTypeDto } from "@/api/types";
import { i18n } from "@/i18n";

export type FieldDef = SchemaFieldDefDto;
export type FieldKind = FieldDef["kind"];
export type FieldValues = Record<string, unknown>;

/** The core field every schema allows at the top level (a UUID or `null`). */
export const FEATURED_IMAGE = "featured_image";
/** Longest `text` value, in characters. */
export const MAX_TEXT = 65_536;
/** Most rows of a `repeater`. */
export const MAX_ROWS = 200;
/** How deep repeaters nest (a repeater inside a repeater, no further). */
export const MAX_REPEATER_DEPTH = 2;

/** The forms `Uuid::parse_str` takes: hyphenated, simple, braced and URN. */
const UUID_FORMS = [
  /^([0-9a-f]{8})-([0-9a-f]{4})-([0-9a-f]{4})-([0-9a-f]{4})-([0-9a-f]{12})$/i,
  /^([0-9a-f]{8})([0-9a-f]{4})([0-9a-f]{4})([0-9a-f]{4})([0-9a-f]{12})$/i,
  /^\{([0-9a-f]{8})-([0-9a-f]{4})-([0-9a-f]{4})-([0-9a-f]{4})-([0-9a-f]{12})\}$/i,
  /^urn:uuid:([0-9a-f]{8})-([0-9a-f]{4})-([0-9a-f]{4})-([0-9a-f]{4})-([0-9a-f]{12})$/i,
];
/** RFC 3339 date-time (chrono's `parse_from_rfc3339`: `T`, `t` or a space between). */
const RFC3339 =
  /^(\d{4})-(\d{2})-(\d{2})[Tt ](\d{2}):(\d{2}):(\d{2})(\.\d+)?([Zz]|[+-](\d{2}):(\d{2}))$/;

export const isObject = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);

/** `v` as a lower-case hyphenated UUID when core would read it as one, else `null`. */
export function uuidOf(v: unknown): string | null {
  if (typeof v !== "string") return null;
  for (const re of UUID_FORMS) {
    const m = re.exec(v);
    if (m) return m.slice(1).join("-").toLowerCase();
  }
  return null;
}

export const isUuid = (v: unknown): v is string => uuidOf(v) !== null;

/** The type's schema, or `null` when it has none (its items take any object). */
export function fieldsFor(type: ContentTypeDto): FieldDef[] | null {
  return type.fields ?? null;
}

/** An item's `fields` as an object (anything else as `{}`). */
export function valuesOf(fields: unknown): FieldValues {
  return isObject(fields) ? fields : {};
}

/** What a new repeater row (or a cleared input) holds: `[]` for a repeater, else `null`. */
export function emptyValue(def: FieldDef): unknown {
  return def.kind === "repeater" ? [] : null;
}

/** A new repeater row: every sub-field empty. */
export function emptyRow(defs: readonly FieldDef[]): FieldValues {
  return Object.fromEntries(defs.map((d) => [d.name, emptyValue(d)]));
}

/**
 * Stored keys the schema does not have: top-level names, and inside repeater rows at any
 * depth as `chapters[].extra` (once per name). `featured_image` is always allowed.
 */
export function staleKeys(defs: readonly FieldDef[], value: FieldValues): string[] {
  const out: string[] = [];
  const walk = (level: readonly FieldDef[], obj: FieldValues, prefix: string, top: boolean) => {
    const names = new Set(level.map((d) => d.name));
    for (const k of Object.keys(obj)) {
      if (top && k === FEATURED_IMAGE) continue;
      if (!names.has(k) && !out.includes(`${prefix}${k}`)) out.push(`${prefix}${k}`);
    }
    for (const d of level) {
      const rows = obj[d.name];
      if (d.kind !== "repeater" || !Array.isArray(rows)) continue;
      for (const row of rows) {
        if (isObject(row)) walk(d.fields ?? [], row, `${prefix}${d.name}[].`, false);
      }
    }
  };
  walk(defs, value, "", true);
  return out;
}

/** What a save sends: `value` without its stale keys, at every level (P39). */
export function toSave(defs: readonly FieldDef[], value: FieldValues): FieldValues {
  const clean = (level: readonly FieldDef[], obj: FieldValues, top: boolean): FieldValues => {
    const out: FieldValues = {};
    for (const [k, v] of Object.entries(obj)) {
      const def = level.find((d) => d.name === k);
      if (!def) {
        if (top && k === FEATURED_IMAGE) out[k] = v;
        continue;
      }
      out[k] =
        def.kind === "repeater" && Array.isArray(v)
          ? (v as unknown[]).map((row) =>
              isObject(row) ? clean(def.fields ?? [], row, false) : row,
            )
          : v;
    }
    return out;
  };
  return clean(defs, value, true);
}

/** JSON with sorted keys; `null` kept (unlike absent), so `{a: null}` ≠ `{}`. */
export function canonicalValue(v: unknown): string {
  const norm = (x: unknown): unknown => {
    if (Array.isArray(x)) return x.map(norm);
    if (!isObject(x)) return x === undefined ? null : x;
    const out: Record<string, unknown> = {};
    for (const k of Object.keys(x).sort()) if (x[k] !== undefined) out[k] = norm(x[k]);
    return out;
  };
  return JSON.stringify(norm(v));
}

export const sameValue = (a: unknown, b: unknown) => canonicalValue(a) === canonicalValue(b);

// --- Validation ------------------------------------------------------------------------

export type IssueCode =
  | "required"
  | "text"
  | "textLong"
  | "richText"
  | "number"
  | "numberComma"
  | "bool"
  | "date"
  | "select"
  | "uuid"
  | "array"
  | "rows"
  | "row"
  | "unknown";

export interface FieldIssue {
  code: IssueCode;
}

const chars = (s: string) => [...s].length;

/** Whether `s` is an RFC 3339 date-time with a real calendar day and clock time. */
export function isRfc3339(s: string): boolean {
  const m = RFC3339.exec(s);
  if (!m) return false;
  const n = (i: number) => Number(m[i] ?? 0);
  const [y, mo, d, h, mi, se, oh, om] = [n(1), n(2), n(3), n(4), n(5), n(6), n(9), n(10)];
  if (mo < 1 || mo > 12 || d < 1 || h > 23 || mi > 59 || se > 60) return false;
  if (oh > 23 || om > 59) return false;
  return d <= new Date(Date.UTC(y, mo, 0)).getUTCDate();
}

/**
 * A number as typed: only `.` separates decimals. A comma (decimal or thousands) is never
 * guessed at: the text is kept and refused with its own message.
 */
export function parseNumber(text: string): number | string | null {
  const s = text.trim();
  if (s === "") return null;
  if (!/^[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?$/.test(s)) return text;
  const n = Number(s);
  return Number.isFinite(n) ? n : text;
}

/** More fields than this go into a card under the body on wide screens. */
export const CARD_THRESHOLD = 4;

/** Where the fields go: nowhere (no schema, nothing to say), the panel, or the card. */
export function fieldsPlacement(
  defs: readonly FieldDef[] | null,
  fields: FieldValues,
  wide: boolean,
): "none" | "panel" | "card" {
  if (defs === null) return "none";
  if (defs.length === 0 && staleKeys(defs, fields).length === 0) return "none";
  return wide && defs.length > CARD_THRESHOLD ? "card" : "panel";
}

function checkValue(def: FieldDef, v: unknown, at: string, out: Record<string, FieldIssue>) {
  const put = (code: IssueCode) => {
    out[at] = { code };
  };
  switch (def.kind) {
    case "text":
      if (typeof v !== "string") put("text");
      else if (chars(v) > MAX_TEXT) put("textLong");
      return;
    case "rich_text":
      if (!isObject(v)) put("richText");
      return;
    case "number":
      if (typeof v === "string" && v.includes(",")) put("numberComma");
      else if (typeof v !== "number" || !Number.isFinite(v)) put("number");
      return;
    case "bool":
      if (typeof v !== "boolean") put("bool");
      return;
    case "date":
      if (typeof v !== "string" || !isRfc3339(v)) put("date");
      return;
    case "select":
      if (typeof v !== "string" || !(def.options ?? []).includes(v)) put("select");
      return;
    case "relation":
    case "media":
      if (!isUuid(v)) put("uuid");
      return;
    case "repeater": {
      if (!Array.isArray(v)) {
        put("array");
        return;
      }
      if (v.length > MAX_ROWS) {
        put("rows");
        return;
      }
      v.forEach((row, i) => {
        const rowAt = `${at}[${i}]`;
        if (!isObject(row)) out[rowAt] = { code: "row" };
        else checkObject(def.fields ?? [], row, rowAt, false, out);
      });
      return;
    }
  }
}

function checkObject(
  defs: readonly FieldDef[],
  obj: FieldValues,
  path: string,
  top: boolean,
  out: Record<string, FieldIssue>,
) {
  for (const key of Object.keys(obj)) {
    const known = defs.some((d) => d.name === key) || (top && key === FEATURED_IMAGE);
    // Top-level unknown keys are stale ones: listed and dropped, not an error.
    if (!known && !top) out[`${path}.${key}`] = { code: "unknown" };
  }
  for (const def of defs) {
    const at = `${path}.${def.name}`;
    const v = obj[def.name];
    if (v === undefined || v === null) {
      if (def.required) out[at] = { code: "required" };
      continue;
    }
    checkValue(def, v, at, out);
  }
}

/** Every problem of `value` against `defs`, by path (stale top-level keys aside). */
export function fieldIssues(
  defs: readonly FieldDef[],
  value: FieldValues,
): Record<string, FieldIssue> {
  const out: Record<string, FieldIssue> = {};
  const image = value[FEATURED_IMAGE];
  if (image !== undefined && image !== null && !isUuid(image)) {
    out[`fields.${FEATURED_IMAGE}`] = { code: "uuid" };
  }
  checkObject(defs, value, "fields", true, out);
  return out;
}

/** The message of an issue (`editor:fields.errors.<code>`). */
export function issueMessage(issue: FieldIssue): string {
  return i18n.t(`editor:fields.errors.${issue.code}`, { max: MAX_TEXT, rows: MAX_ROWS });
}

/** `fieldIssues` as messages, by path (`fields.price` → "Must be a number"). */
export function validateFields(
  defs: readonly FieldDef[],
  value: FieldValues,
): Record<string, string> {
  return Object.fromEntries(
    Object.entries(fieldIssues(defs, value)).map(([path, issue]) => [path, issueMessage(issue)]),
  );
}

const PATH = /^(fields(?:\.[a-z][a-z0-9_]*|\[\d+\])+): (.+)$/s;

/**
 * A `422`'s field errors by input path: `fields` errors carry their path at the start of
 * `detail` (`fields.chapters[2].title: required`); one without a path stays under
 * `fields`. Other members (`seo.title`) keep their `field`.
 */
export function serverFieldErrors(error: unknown): Record<string, string> {
  if (!(error instanceof ApiError)) return {};
  const out: Record<string, string> = {};
  for (const e of error.problem.errors ?? []) {
    if (!e.field) continue;
    if (e.field === "fields") {
      const m = PATH.exec(e.detail);
      const path = m?.[1] ?? "fields";
      const message = m?.[2] ?? e.detail;
      if (!(path in out)) out[path] = message;
    } else if (e.field.startsWith("fields") || e.field.startsWith("seo.")) {
      // Core starts the message with the path too (`seo.title: …`).
      const prefix = `${e.field}: `;
      const message = e.detail.startsWith(prefix) ? e.detail.slice(prefix.length) : e.detail;
      if (!(e.field in out)) out[e.field] = message;
    }
  }
  return out;
}
