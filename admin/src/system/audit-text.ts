// The audit log's words: the actions core writes (`ferrite-core`, every `NewAudit`) as
// sentences, their areas for the action filter, the target kinds, and a row's `details`
// as key-value text. An action this list does not know (a newer node, a later
// milestone) is shown as it is, never looked up, so it can never be a missing key.
//
// Kept in step with core by hand (no endpoint lists the actions): `user.*`
// (`users.rs`, `auth/reset.rs`), `session.revoke`, `token.*`, `totp.*`
// (`auth/service.rs`), `settings.update` (`options.rs`), `mail.test`, `job.retry`
// (`system.rs`), `plugin.*` and `theme.*` (`extensions/admin.rs`), `type.*` and
// `taxonomy.*` (`types.rs` `announce`), `webhook.*` (`webhooks/service.rs`,
// `webhooks/deliver.rs`).

import type { TFunction } from "i18next";

export const AUDIT_ACTIONS = [
  "user.create",
  "user.update",
  "user.delete",
  "user.password",
  "user.password_reset",
  "user.totp_disable",
  "session.revoke",
  "token.create",
  "token.revoke",
  "totp.enable",
  "totp.disable",
  "settings.update",
  "mail.test",
  "job.retry",
  "plugin.install",
  "plugin.enable",
  "plugin.disable",
  "plugin.delete",
  "plugin.purge",
  "plugin.settings",
  "theme.install",
  "theme.activate",
  "theme.settings",
  "type.create",
  "type.update",
  "type.delete",
  "taxonomy.create",
  "taxonomy.update",
  "taxonomy.delete",
  "webhook.create",
  "webhook.update",
  "webhook.enable",
  "webhook.disable",
  "webhook.delete",
  "webhook.rotate_secret",
  "webhook.test",
  "webhook.redeliver",
  "webhook.auto_disabled",
] as const;
export type AuditAction = (typeof AUDIT_ACTIONS)[number];

/** The areas of the known actions (`<area>.<verb>`), in the filter's order. */
export const AUDIT_AREAS = [
  "user",
  "session",
  "token",
  "totp",
  "settings",
  "mail",
  "job",
  "plugin",
  "theme",
  "type",
  "taxonomy",
  "webhook",
] as const;
export type AuditArea = (typeof AUDIT_AREAS)[number];

/** The target kinds core writes. */
export const TARGET_KINDS = [
  "user",
  "api_token",
  "settings",
  "job",
  "plugin",
  "theme",
  "type",
  "taxonomy",
  "webhook",
] as const;
export type TargetKind = (typeof TARGET_KINDS)[number];

const isOneOf = <T extends string>(list: readonly T[], v: string): v is T =>
  (list as readonly string[]).includes(v);

export const isAuditAction = (action: string): action is AuditAction =>
  isOneOf(AUDIT_ACTIONS, action);

/** The API's prefix form: an area and a dot (`user.`). */
export const isAuditPrefix = (action: string): boolean => /^[a-z0-9_]{1,99}\.$/.test(action);

/** A known action's sentence; anything else as it is. */
export function auditActionText(t: TFunction<"admin">, action: string): string {
  if (!isAuditAction(action)) return action;
  const [area, verb] = action.split(".") as [string, string];
  return t(`system.audit.actions.${area}.${verb}` as `system.audit.actions.user.create`);
}

/** The known area of `action` (or of a prefix), else `null`. */
export function auditAreaOf(action: string): AuditArea | null {
  const area = action.split(".")[0] ?? "";
  return action.includes(".") && isOneOf(AUDIT_AREAS, area) ? area : null;
}

/** A known area's actions, in list order. */
export function actionsOf(area: AuditArea): AuditAction[] {
  return AUDIT_ACTIONS.filter((a) => a.startsWith(`${area}.`));
}

export function targetKindText(t: TFunction<"admin">, kind: string): string {
  return isOneOf(TARGET_KINDS, kind) ? t(`system.audit.targets.${kind}`) : kind;
}

/** `details` as `[key, text]` pairs (arrays of plain values joined, objects as JSON). */
export function detailEntries(details: unknown): [string, string][] {
  if (details === null || typeof details !== "object") return [];
  return Object.entries(details as Record<string, unknown>).map(([key, value]) => [
    key,
    valueText(value),
  ]);
}

function valueText(value: unknown): string {
  if (value === null || value === undefined) return "—";
  if (typeof value === "string") return value;
  if (typeof value === "number" || typeof value === "boolean") return String(value);
  if (Array.isArray(value) && value.every((v) => v === null || typeof v !== "object")) {
    return value.map((v) => String(v)).join(", ");
  }
  return JSON.stringify(value);
}
