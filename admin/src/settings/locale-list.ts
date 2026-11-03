// `site.locales` as the Languages group edits it: rows of `{tag, label}`, the default
// first, and the checks core makes (`ferrite_domain::locale`), before a save.

import { canonicalTag } from "@/lib/locales";
import type { FieldSpec, Problem } from "./fields";

export interface LocaleRow {
  tag: string;
  label: string;
}

/** Core's limits (`ferrite_domain::locale`). */
export const MAX_LOCALES = 20;
export const MAX_LABEL = 50;

/** The stored list, else the one-locale default from `site.locale`. */
export function readLocales(stored: unknown): LocaleRow[] {
  if (!Array.isArray(stored)) return [];
  return stored.flatMap((e: unknown) => {
    if (typeof e !== "object" || e === null) return [];
    const { tag, label } = e as { tag?: unknown; label?: unknown };
    return typeof tag === "string" ? [{ tag, label: typeof label === "string" ? label : "" }] : [];
  });
}

/** What the form checks before a save (the server checks again). */
export function checkLocales(rows: LocaleRow[]): Problem | null {
  if (rows.length === 0) return { code: "localeList", detail: "empty" };
  if (rows.length > MAX_LOCALES) return { code: "localeList", detail: "tooMany" };
  const seen = new Set<string>();
  for (const r of rows) {
    const tag = canonicalTag(r.tag);
    if (!tag) return { code: "localeList", detail: "badTag" };
    const prefix = tag.toLowerCase();
    if (seen.has(prefix)) return { code: "localeList", detail: "duplicate" };
    seen.add(prefix);
    if (Array.from(r.label.trim()).length > MAX_LABEL) {
      return { code: "localeList", detail: "labelTooLong" };
    }
  }
  return null;
}

export const localesSpec: FieldSpec<LocaleRow[]> = {
  read: readLocales,
  write: (rows) => rows.map((r) => ({ tag: r.tag, label: r.label.trim() })),
  check: checkLocales,
};
