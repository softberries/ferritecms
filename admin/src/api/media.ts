import { api, unwrap } from "./client";
import { ifMatch } from "./content";
import type { MediaListFilters } from "./keys";
import { ApiError } from "./problem";
import type { SchemaMediaDto, SchemaMediaList, SchemaMediaPatchInput } from "./schema";

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

export type MediaKind = "image" | "video" | "audio" | "document";

/** `GET /media`: one page of 50 (the server default). */
export function listMedia(
  query: MediaListFilters & { cursor?: string } = {},
  options: { signal?: AbortSignal } = {},
): Promise<SchemaMediaList> {
  // A blank `q` means no search (and `q` cannot combine with `author` or `trash`).
  const { q, ...rest } = query;
  const clean = q === undefined || q.trim() === "" ? rest : { ...rest, q };
  return unwrap(
    api.GET("/api/v1/media", {
      params: { query: clean },
      ...(options.signal ? { signal: options.signal } : {}),
    }),
  );
}

export function patchMedia(
  id: string,
  version: number,
  body: SchemaMediaPatchInput,
): Promise<MediaDto> {
  return unwrap(
    api.PATCH("/api/v1/media/{id}", {
      params: { path: { id }, header: ifMatch(version) },
      body,
    }),
  );
}

/** Moves the item to the trash. */
export async function trashMedia(id: string, version: number): Promise<void> {
  await unwrap(
    api.DELETE("/api/v1/media/{id}", { params: { path: { id }, header: ifMatch(version) } }),
  );
}

export function restoreMedia(id: string, version: number): Promise<MediaDto> {
  return unwrap(
    api.POST("/api/v1/media/{id}/restore", { params: { path: { id }, header: ifMatch(version) } }),
  );
}

/** Deletes a trashed item for good (`force=true`). */
export async function deleteMedia(id: string, version: number): Promise<void> {
  await unwrap(
    api.DELETE("/api/v1/media/{id}", {
      params: { path: { id }, query: { force: true }, header: ifMatch(version) },
    }),
  );
}
