// Parsing helpers shared by the editor's custom fields and the settings form.

/** The forms `Uuid::parse_str` takes: hyphenated, simple, braced and URN. */
const UUID_FORMS = [
  /^([0-9a-f]{8})-([0-9a-f]{4})-([0-9a-f]{4})-([0-9a-f]{4})-([0-9a-f]{12})$/i,
  /^([0-9a-f]{8})([0-9a-f]{4})([0-9a-f]{4})([0-9a-f]{4})([0-9a-f]{12})$/i,
  /^\{([0-9a-f]{8})-([0-9a-f]{4})-([0-9a-f]{4})-([0-9a-f]{4})-([0-9a-f]{12})\}$/i,
  /^urn:uuid:([0-9a-f]{8})-([0-9a-f]{4})-([0-9a-f]{4})-([0-9a-f]{4})-([0-9a-f]{12})$/i,
];
/** `v` as a lower-case hyphenated UUID when core would read it as one, else `null`. */
export function uuidOf(v: unknown): string | null {
  if (typeof v !== "string") return null;
  for (const re of UUID_FORMS) {
    const m = re.exec(v);
    if (m) return m.slice(1).join("-").toLowerCase();
  }
  return null;
}

/**
 * A number as typed: only `.` separates decimals. A comma (decimal or thousands) is never
 * guessed at: the text is kept and refused with its own message.
 */
export function parseNumber(text: string): number | string | null {
  const s = text.trim();
  if (s === "") return null;
  if (!/^[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?$/.test(s)) return text;
  const n = Number(s);
  return Number.isFinite(n) ? n : text;
}
