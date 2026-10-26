// The comment author's website as a link (Global Constraints: untrusted text is text).

/** `url` when it is an absolute `http:`/`https:` address, else `null` (shown as text). */
export function authorLink(url: string): string | null {
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    return null;
  }
  return parsed.protocol === "http:" || parsed.protocol === "https:" ? url : null;
}
