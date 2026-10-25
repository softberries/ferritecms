import { api, unwrap } from "./client";
import { ApiError } from "./problem";
import type { SchemaMediaDto } from "./schema";

export type MediaDto = SchemaMediaDto;

export function getMedia(id: string): Promise<MediaDto> {
  return unwrap(api.GET("/api/v1/media/{id}", { params: { path: { id } } }));
}

/** The variants an editor shows, largest first; the original when none exists yet. */
const DISPLAY_VARIANTS = ["large", "medium"];

/** The URL to show for a media item: its `large` (else `medium`) variant, else the original. */
export function displayUrl(media: MediaDto): string {
  for (const name of DISPLAY_VARIANTS) {
    const v = media.variants.find((x) => x.name === name);
    if (v) return v.url;
  }
  return media.url;
}

/**
 * `ferrite:media/<id>` → the URL to show; `null` when the item is gone (404) or trashed.
 * Other failures (offline, 5xx) reject, so the caller can tell "missing" from "failed".
 */
export async function resolveMediaUrl(id: string): Promise<string | null> {
  try {
    const media = await getMedia(id);
    return media.status === "trash" ? null : displayUrl(media);
  } catch (e) {
    if (e instanceof ApiError && (e.status === 404 || e.status === 400)) return null;
    throw e;
  }
}
