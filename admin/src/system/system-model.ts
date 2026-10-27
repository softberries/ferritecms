// The System screen's rules that are not components: its tabs, its URL state, the
// schema verdict in plain words, a theme origin's name and the clock-skew flag.

import type { TFunction } from "i18next";
import type { SchemaNodeThemeOrigin, SchemaSchemaStatusDto } from "@/api/schema";

export const SYSTEM_TABS = ["status", "nodes", "jobs", "audit"] as const;
export type SystemTab = (typeof SYSTEM_TABS)[number];

export const isSystemTab = (s: string): s is SystemTab =>
  (SYSTEM_TABS as readonly string[]).includes(s);

export const JOB_STATUSES = ["queued", "running", "done", "dead"] as const;
export type JobStatus = (typeof JOB_STATUSES)[number];

/** Jobs and audit rows a page (constraints: exact values). */
export const SYSTEM_PAGE = 50;

/**
 * `/admin/system/<tab>?…`: the jobs filters (`status`, `kind`) and the audit filters
 * (`actor`, `action`, `target_kind`, `target_id`), each checked as the API checks it.
 * Invalid members are dropped; a tab ignores the others' members.
 */
export interface SystemSearch {
  status?: JobStatus;
  kind?: string;
  actor?: string;
  action?: string;
  target_kind?: string;
  target_id?: string;
}

export type SystemSearchPatch = { [K in keyof SystemSearch]?: SystemSearch[K] | undefined };

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const KIND = /^[a-z0-9._-]{1,64}$/;
const ACTION = /^[a-z0-9_.]{1,100}$/;
const TARGET_KIND = /^[a-z0-9_]{1,100}$/;

/** The router's codec reads `?target_id=42` as a number: text again. */
const text = (v: unknown): string | undefined =>
  typeof v === "string" ? v : typeof v === "number" ? String(v) : undefined;

/** `validateSearch` of the route. */
export function parseSystemSearch(raw: Record<string, unknown>): SystemSearch {
  const out: SystemSearch = {};
  const status = text(raw.status);
  if (status && (JOB_STATUSES as readonly string[]).includes(status)) {
    out.status = status as JobStatus;
  }
  const kind = text(raw.kind);
  if (kind && KIND.test(kind)) out.kind = kind;
  const actor = text(raw.actor);
  if (actor && UUID.test(actor)) out.actor = actor.toLowerCase();
  const action = text(raw.action);
  if (action && ACTION.test(action) && !action.startsWith(".")) out.action = action;
  const targetKind = text(raw.target_kind);
  if (targetKind && TARGET_KIND.test(targetKind)) {
    out.target_kind = targetKind;
    const targetId = text(raw.target_id)?.trim();
    if (targetId && Array.from(targetId).length <= 255) out.target_id = targetId;
  }
  return out;
}

export type Tone = "ok" | "warn" | "bad";

export interface Verdict {
  tone: Tone;
  title: string;
  body: string;
}

/** The schema verdict of `GET /system/status` in plain words. */
export function schemaVerdict(t: TFunction<"admin">, schema: SchemaSchemaStatusDto): Verdict {
  const values = { db: schema.db_version, binary: schema.binary_version };
  switch (schema.verdict as string) {
    case "up_to_date":
      return {
        tone: "ok",
        title: t("system.schema.upToDate.title"),
        body: t("system.schema.upToDate.body", values),
      };
    case "needs_up": {
      const count = schema.pending.length;
      return {
        tone: "warn",
        title: t("system.schema.needsUp.title", { count }),
        body: t("system.schema.needsUp.body", { ...values, count }),
      };
    }
    case "newer_schema_ok":
      return {
        tone: "warn",
        title: t("system.schema.newer.title"),
        body: t("system.schema.newer.body", values),
      };
    case "incompatible":
      return {
        tone: "bad",
        title: t("system.schema.incompatible.title"),
        body: t("system.schema.incompatible.body", values),
      };
    default:
      return { tone: "warn", title: schema.verdict, body: t("system.schema.unknown", values) };
  }
}

/** A node's theme origin: built in, a folder, an installed bundle; else as sent. */
export function themeOriginText(
  t: TFunction<"admin">,
  origin: SchemaNodeThemeOrigin | null | undefined,
): string | null {
  if (origin === null || origin === undefined) return null;
  if (typeof origin === "object") return t("system.nodes.origin.artifact");
  if (origin === "embedded") return t("system.nodes.origin.embedded");
  if (origin === "folder") return t("system.nodes.origin.folder");
  return origin;
}

/** Clock skew past this (either way) is flagged (07 §3 System). */
export const SKEW_LIMIT_MS = 1000;

export function skewFlag(skew: number | null | undefined): boolean {
  return typeof skew === "number" && Math.abs(skew) > SKEW_LIMIT_MS;
}
