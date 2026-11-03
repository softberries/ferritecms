// The state of the "Override approval" disclosure (`override-field.tsx`, design 15 §6.5).

import { MAX_OVERRIDE_REASON } from "@/api/releases";

export interface OverrideState {
  on: boolean;
  reason: string;
}

export const NO_OVERRIDE: OverrideState = { on: false, reason: "" };

/** The reason to send, `undefined` when no override is asked for. */
export const overrideReason = (s: OverrideState): string | undefined =>
  s.on && s.reason.trim() !== "" ? s.reason.trim() : undefined;

/** An override was asked for but has no usable reason yet. */
export const overrideIncomplete = (s: OverrideState): boolean =>
  s.on && (s.reason.trim() === "" || s.reason.trim().length > MAX_OVERRIDE_REASON);
