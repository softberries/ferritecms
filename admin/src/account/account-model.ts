// The account screen's logic without components: which scopes a new token may take, its
// expiry as RFC 3339, the TOTP key's grouping, the password form's schema, and how a
// failed write becomes what a form shows. The writes are plain awaited calls, not
// TanStack mutations, so a password or a secret never reaches the mutation cache
// (Review Focus 3).

import { reportApiError } from "@/api/client";
import { ApiError } from "@/api/problem";
import { SCOPE_AREAS, type ScopeArea } from "@/api/tokens";
import { type FormFailure, genericFailure } from "@/auth/errors";
import { changeSchema } from "@/auth/schemas";
import { z } from "@/lib/zod";

const DAY_MS = 86_400_000;

export const EXPIRY_CHOICES = ["never", "30", "90", "365"] as const;
export type Expiry = (typeof EXPIRY_CHOICES)[number];

/** An area the session holds a scope of, and whether that includes `write`. */
export interface OfferedArea {
  area: ScopeArea;
  write: boolean;
}

/** The areas to offer, in `SCOPE_AREAS` order: those `held` has `read` or `write` of. */
export function offeredScopes(held: readonly string[]): OfferedArea[] {
  const has = new Set(held);
  return SCOPE_AREAS.flatMap((area) => {
    const write = has.has(`${area}:write`);
    return write || has.has(`${area}:read`) ? [{ area, write }] : [];
  });
}

/** `expires_at` for an expiry choice: RFC 3339 UTC, whole seconds; `null` for Never. */
export function expiresAt(choice: Expiry, now: number = Date.now()): string | null {
  if (choice === "never") return null;
  return new Date(now + Number(choice) * DAY_MS).toISOString().replace(/\.\d{3}Z$/, "Z");
}

/** The base32 key in groups of four (constraints: exact values). */
export function groupedSecret(secret: string): string {
  return secret.match(/.{1,4}/g)?.join(" ") ?? secret;
}

/** The password form: `changeSchema` plus the repeated new password. */
export const passwordSchema = changeSchema
  .extend({ repeat: z.string() })
  .refine((v) => v.new === v.repeat, { message: "policy.mismatch", path: ["repeat"] });

/**
 * A failed write the form shows itself: the app's handler still sends a `401` to sign
 * in (no toast); the result is the form-level failure (a `429` with its wait, no
 * connection, anything else).
 */
export function writeFailure(error: unknown): FormFailure {
  reportApiError(error);
  return genericFailure(error);
}

/** `true` for a `403` (the password the request carried is wrong). */
export function isWrongPassword(error: unknown): boolean {
  return error instanceof ApiError && error.status === 403;
}

/** The `422` field messages of `error`, or `null` when it is not one. */
export function fieldErrorsOf(error: unknown): Record<string, string> | null {
  return error instanceof ApiError && error.status === 422 ? error.fieldErrors() : null;
}

/** `errors` without the message of `key` (a field the user has just changed). */
export function withoutKey<K extends string>(
  errors: Partial<Record<K, string>>,
  key: K,
): Partial<Record<K, string>> {
  const next = { ...errors };
  delete next[key];
  return next;
}
