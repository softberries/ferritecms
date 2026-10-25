// The sign-in forms' schemas (react-hook-form + zod, 07 §3). Messages are keys of the
// `auth` namespace, translated where they are shown (`fieldMessage`).

import type { TFunction } from "i18next";
import type { FieldError } from "react-hook-form";
import { z } from "@/lib/zod";
import { MAX_PASSWORD_BYTES, MIN_PASSWORD_CHARS, byteLength, charCount } from "./errors";

/** Every key a schema uses as its message. */
export type FieldMessageKey =
  | "login.required"
  | "login.passwordRequired"
  | "change.currentRequired"
  | "policy.tooShort"
  | "policy.tooLong"
  | "policy.mismatch";

const message = (key: FieldMessageKey) => ({ message: key });

/** A new password against the policy (07 §2): ≥ 12 characters, ≤ 1024 bytes. */
const newPassword = z
  .string()
  .refine((v) => charCount(v) >= MIN_PASSWORD_CHARS, message("policy.tooShort"))
  .refine((v) => byteLength(v) <= MAX_PASSWORD_BYTES, message("policy.tooLong"));

export const loginSchema = z.object({
  login: z.string().trim().min(1, message("login.required")),
  password: z.string().min(1, message("login.passwordRequired")),
});

export const forgotSchema = z.object({
  login: z.string().trim().min(1, message("login.required")),
});

export const changeSchema = z.object({
  current: z.string().min(1, message("change.currentRequired")),
  new: newPassword,
});

export const resetSchema = z
  .object({ password: newPassword, repeat: z.string() })
  .refine((v) => v.password === v.repeat, { ...message("policy.mismatch"), path: ["repeat"] });

/**
 * The text of a field error: a schema's message key translated, or a server's own
 * detail (set with `type: "server"`) as sent.
 */
export function fieldMessage(t: TFunction<"auth">, error: FieldError | undefined): string | null {
  if (!error?.message) return null;
  if (error.type === "server") return error.message;
  return t(error.message as FieldMessageKey);
}
