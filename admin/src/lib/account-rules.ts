// Core's rules for account fields, checked in the browser before a write (the server
// checks again): display names (`normalize_display_name`), e-mail shape
// (`normalize_email`, loosely) and the password policy (07 §2).

import { MAX_PASSWORD_BYTES, MIN_PASSWORD_CHARS, byteLength, charCount } from "@/auth/errors";

/** Core's limit on a display name, in characters. */
export const MAX_DISPLAY_NAME = 100;

/** What is wrong with a display name (already trimmed), or `null`. */
export function displayNameProblem(name: string): "required" | "long" | null {
  if (name === "") return "required";
  if (Array.from(name).length > MAX_DISPLAY_NAME) return "long";
  return null;
}

/** One `@` with text on both sides and no whitespace (the server checks the rest). */
export function isEmailShape(email: string): boolean {
  return /^[^\s@]+@[^\s@]+$/.test(email);
}

/** What is wrong with a new password against the policy, or `null`. */
export function passwordProblem(password: string): "short" | "long" | null {
  if (charCount(password) < MIN_PASSWORD_CHARS) return "short";
  if (byteLength(password) > MAX_PASSWORD_BYTES) return "long";
  return null;
}
