// How the sign-in screens turn a failed call into what the form shows, and the
// countdown a `429` starts.

import { useEffect, useEffectEvent, useState } from "react";
import { NetworkError } from "@/api/client";
import { ApiError } from "@/api/problem";

/** A form-level failure: a message key of the `auth` namespace, or a rate limit. */
export type FormFailure =
  | { kind: "message"; key: AuthMessageKey }
  | { kind: "rate-limited"; seconds: number | null; id: number };

export type AuthMessageKey =
  | "errors.network"
  | "errors.generic"
  | "login.invalid"
  | "totp.invalid"
  | "change.currentWrong"
  | "forgot.mailUnavailable";

let failures = 0;

/** `429` → a rate limit of `Retry-After` seconds (`null` when not sent). */
export function rateLimit(error: unknown): FormFailure | null {
  if (!(error instanceof ApiError) || error.status !== 429) return null;
  failures += 1;
  return { kind: "rate-limited", seconds: error.retryAfter, id: failures };
}

/** The common failures: rate limit, no response, anything else (never the raw message). */
export function genericFailure(error: unknown): FormFailure {
  const limited = rateLimit(error);
  if (limited) return limited;
  if (error instanceof NetworkError) return { kind: "message", key: "errors.network" };
  return { kind: "message", key: "errors.generic" };
}

/**
 * A countdown from `seconds` that ticks once a second and calls `onDone` at zero. The
 * component using it is keyed by the failure, so each `429` starts a fresh one.
 */
export function useCountdown(seconds: number, onDone: () => void): number {
  const [left, setLeft] = useState(seconds);
  const done = useEffectEvent(onDone);
  useEffect(() => {
    if (left <= 0) {
      done();
      return;
    }
    const id = setTimeout(() => {
      setLeft((n) => n - 1);
    }, 1000);
    return () => {
      clearTimeout(id);
    };
  }, [left]);
  return Math.max(0, left);
}

/** Unicode code points, as the server counts a password's characters. */
export function charCount(text: string): number {
  return [...text].length;
}

/** UTF-8 bytes (the policy's upper bound is 1024 bytes). */
export function byteLength(text: string): number {
  return new TextEncoder().encode(text).length;
}

export const MIN_PASSWORD_CHARS = 12;
export const MAX_PASSWORD_BYTES = 1024;
