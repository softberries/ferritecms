// Core's rules for a type's own name and label (`crates/ferrite-core/src/types.rs`:
// `check_type_name`, `check_label`), for feedback before the request; reserved router
// words and names already taken are the server's to say.

/** Longest type name (`MAX_TYPE_NAME`). */
export const MAX_TYPE_NAME = 32;
/** Longest type label, in characters, after trimming (`MAX_TYPE_LABEL`). */
export const MAX_TYPE_LABEL = 100;

/** `[a-z][a-z0-9_-]{0,31}`. */
export const validTypeName = (name: string) =>
  /^[a-z][a-z0-9_-]*$/.test(name) && name.length <= MAX_TYPE_NAME;

// Control characters (Rust's `char::is_control`: C0, DEL and C1).
// eslint-disable-next-line no-control-regex
const CONTROL = /[\u0000-\u001f\u007f-\u009f]/;

/** `message` when `label` breaks the rule (1..=100 characters after trimming, no control). */
export function labelProblem(label: string, message: string): string | null {
  const trimmed = label.trim();
  const len = [...trimmed].length;
  return len === 0 || len > MAX_TYPE_LABEL || CONTROL.test(trimmed) ? message : null;
}
