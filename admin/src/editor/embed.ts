// Video URLs of the `embed` node (M3 P11): the admin twin of the core's `blocks::embed_id`
// (`crates/ferrite-core/src/blocks/url.rs`). Both are pinned to the URL cases in
// `crates/ferrite-core/tests/fixtures/editor-schema.json`, so every embed the editor
// makes plays on the site.

/** The providers an `embed` node may name. */
export const EMBED_PROVIDERS = ["youtube", "vimeo"] as const;

export type EmbedProvider = (typeof EMBED_PROVIDERS)[number];

const YOUTUBE_HOSTS = new Set([
  "youtube.com",
  "www.youtube.com",
  "m.youtube.com",
  "youtube-nocookie.com",
  "www.youtube-nocookie.com",
]);
const VIMEO_HOSTS = new Set(["vimeo.com", "www.vimeo.com", "player.vimeo.com"]);
const YOUTUBE_ID = /^[A-Za-z0-9_-]{11}$/;
const VIMEO_ID = /^[0-9]{1,20}$/;

/**
 * The provider and video id of a YouTube (`/watch?v=`, `youtu.be/`, `/embed/`,
 * `/shorts/`; an 11-character id) or Vimeo URL (the first all-digit path segment), or
 * `null`. Only `http(s)` URLs on the providers' own hosts, without user info, count.
 */
export function parseEmbed(raw: string): { provider: EmbedProvider; id: string } | null {
  let url: URL;
  try {
    url = new URL(raw.trim());
  } catch {
    return null;
  }
  if (url.protocol !== "http:" && url.protocol !== "https:") return null;
  if (url.username !== "" || url.password !== "") return null;
  const host = url.hostname;
  const segments = url.pathname.split("/").filter(Boolean);
  if (host === "youtu.be" || YOUTUBE_HOSTS.has(host)) {
    let id: string | null | undefined;
    if (host === "youtu.be") id = segments[0];
    else if (segments[0] === "watch" && segments.length === 1) id = url.searchParams.get("v");
    else if (segments[0] === "embed" || segments[0] === "shorts") id = segments[1];
    return id && YOUTUBE_ID.test(id) ? { provider: "youtube", id } : null;
  }
  if (VIMEO_HOSTS.has(host)) {
    const id = segments.find((s) => VIMEO_ID.test(s));
    return id ? { provider: "vimeo", id } : null;
  }
  return null;
}

/** The URL an `embed` node stores for a video: its provider's plain watch page. */
export function canonicalEmbedUrl(provider: EmbedProvider, id: string): string {
  return provider === "youtube"
    ? `https://www.youtube.com/watch?v=${id}`
    : `https://vimeo.com/${id}`;
}

/** The `embed` node attributes for a pasted video URL, or `null` if it is not one. */
export function embedFromUrl(raw: string): { provider: EmbedProvider; url: string } | null {
  const parsed = parseEmbed(raw);
  return parsed
    ? { provider: parsed.provider, url: canonicalEmbedUrl(parsed.provider, parsed.id) }
    : null;
}
