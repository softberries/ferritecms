// How each site setting is edited: what the form shows for the stored value (`read`),
// what it sends (`write`; `null` resets to the default) and core's limits, checked
// before the save (`ferrite-core/src/settings.rs`, `SETTINGS`). The server checks
// again; its `422` shows at the field.

import type { SettingsDto } from "@/api/settings";
import {
  DEFAULT_PAGE_STRUCTURE,
  DEFAULT_POST_STRUCTURE,
  type StructureKind,
  type StructureProblem,
  checkStructure,
} from "./permalink-preview";

export type SettingKey = Extract<keyof SettingsDto, string>;

/** Core's limits and defaults. */
export const LIMITS = {
  /** `MAX_TITLE`, characters. */
  title: 200,
  /** `MAX_DESCRIPTION`, characters. */
  description: 1000,
  /** `MAX_LOCALE`, ASCII characters. */
  locale: 35,
  /** `reading.per_page` and `comments.per_page`. */
  perPage: { min: 1, max: 100 },
} as const;

export const DEFAULTS = {
  /** `site.rs` `DEFAULT_PER_PAGE`. */
  readingPerPage: 10,
  /** `COMMENTS_PER_PAGE_DEFAULT`. */
  commentsPerPage: 50,
  /** `COMMENTS_ENABLED_DEFAULT`. */
  commentsEnabled: true,
  /** `COMMENTS_MODERATION_DEFAULT`. */
  moderation: "known",
  discourageIndexing: false,
} as const;

export const MODERATION_CHOICES = ["all", "known"] as const;
export type Moderation = (typeof MODERATION_CHOICES)[number];

/** A refusal before the save: a key under `settings.errors` and its values. */
export type Problem =
  | { code: "tooLong"; max: number }
  | { code: "control" }
  | { code: "locale" }
  | { code: "integer"; min: number; max: number }
  | { code: "pickPage" }
  | { code: "permalink"; detail: StructureProblem };

export interface FieldSpec<V> {
  /** The form's value for a stored one (`null`: unset, the default). */
  read: (stored: unknown) => V;
  /** What `PATCH` sends for the form's value. */
  write: (value: V) => unknown;
  check?: (value: V) => Problem | null;
}

/** A specs record's form values. */
export type DraftOf<S> = { [K in keyof S]: S[K] extends FieldSpec<infer V> ? V : never };

/** Characters as core counts them (Unicode scalar values). */
export const charCount = (s: string): number => Array.from(s).length;

/** Rust's `char::is_control` (the Cc category); `breaks` lets `\n`, `\r` and `\t` pass. */
function hasControl(s: string, breaks: boolean): boolean {
  for (const ch of s) {
    const cp = ch.codePointAt(0) ?? 0;
    const control = cp <= 0x1f || (cp >= 0x7f && cp <= 0x9f);
    if (control && !(breaks && (ch === "\n" || ch === "\r" || ch === "\t"))) return true;
  }
  return false;
}

/**
 * Free text. An empty value (after trimming, when `trim`) is sent as `null`: the site
 * then uses `ferrite.toml`'s value.
 */
export function textSpec(opts: { max: number; multiline: boolean }): FieldSpec<string> {
  return {
    read: (stored) => (typeof stored === "string" ? stored : ""),
    write: (value) => {
      const v = opts.multiline ? value : value.trim();
      return v.trim() === "" ? null : v;
    },
    check: (value) => {
      const v = opts.multiline ? value : value.trim();
      if (charCount(v) > opts.max) return { code: "tooLong", max: opts.max };
      if (hasControl(v, opts.multiline)) return { code: "control" };
      return null;
    },
  };
}

/** `site.locale`: 1..=35 ASCII letters, digits, `-` or `_`; empty is `ferrite.toml`'s. */
export const localeSpec: FieldSpec<string> = {
  read: (stored) => (typeof stored === "string" ? stored : ""),
  write: (value) => (value.trim() === "" ? null : value.trim()),
  check: (value) => {
    const v = value.trim();
    if (v === "") return null;
    return v.length <= LIMITS.locale && /^[A-Za-z0-9_-]+$/.test(v) ? null : { code: "locale" };
  },
};

/** An integer in `min..=max`, typed as text; empty is the default. */
export function intSpec(min: number, max: number): FieldSpec<string> {
  return {
    read: (stored) => (typeof stored === "number" ? String(stored) : ""),
    write: (value) => {
      const v = value.trim();
      if (v === "") return null;
      return /^[+-]?\d+$/.test(v) ? Number(v) : v;
    },
    check: (value) => {
      const v = value.trim();
      if (v === "") return null;
      const n = Number(v);
      return /^[+-]?\d+$/.test(v) && n >= min && n <= max ? null : { code: "integer", min, max };
    },
  };
}

export function boolSpec(fallback: boolean): FieldSpec<boolean> {
  return {
    read: (stored) => (typeof stored === "boolean" ? stored : fallback),
    write: (value) => value,
  };
}

export const moderationSpec: FieldSpec<Moderation> = {
  read: (stored) =>
    (MODERATION_CHOICES as readonly unknown[]).includes(stored)
      ? (stored as Moderation)
      : DEFAULTS.moderation,
  write: (value) => value,
};

/** The front page: the post listing, or a page (picked by id). */
export type FrontPage = { kind: "posts" } | { kind: "page"; id: string | null };

export const frontPageSpec: FieldSpec<FrontPage> = {
  read: (stored) =>
    typeof stored === "string" && stored !== "" ? { kind: "page", id: stored } : { kind: "posts" },
  write: (value) => (value.kind === "posts" ? null : value.id),
  check: (value) => (value.kind === "page" && value.id === null ? { code: "pickPage" } : null),
};

/** A permalink structure; unset reads as core's default. */
export function structureSpec(kind: StructureKind): FieldSpec<string> {
  const fallback = kind === "post" ? DEFAULT_POST_STRUCTURE : DEFAULT_PAGE_STRUCTURE;
  return {
    read: (stored) => (typeof stored === "string" && stored !== "" ? stored : fallback),
    write: (value) => value.trim(),
    check: (value) => {
      const detail = checkStructure(value.trim(), kind);
      return detail ? { code: "permalink", detail } : null;
    },
  };
}
