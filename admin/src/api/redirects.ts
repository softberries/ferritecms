import { api, unwrap } from "./client";
import type {
  SchemaImportReportDto,
  SchemaRedirectDto,
  SchemaRedirectInput,
  SchemaRedirectList,
  SchemaRedirectPatch,
} from "./schema";

export type RedirectDto = SchemaRedirectDto;

/** The CSV download (a plain link; the session cookie authorises it). */
export const exportRedirectsUrl = "/api/v1/redirects/export";

/** `GET /redirects`; a blank `q` is left out (the API takes it as no search too). */
export function listRedirects(
  query: { q?: string; cursor?: string; limit?: number } = {},
  options: { signal?: AbortSignal } = {},
): Promise<SchemaRedirectList> {
  const { q, ...rest } = query;
  const trimmed = q?.trim() ?? "";
  return unwrap(
    api.GET("/api/v1/redirects", {
      params: { query: trimmed === "" ? rest : { ...rest, q: trimmed } },
      ...(options.signal ? { signal: options.signal } : {}),
    }),
  );
}

/** Most redirects one `GET /redirects` page holds (the API's page maximum). */
const MAX_PAGE = 200;
/** Longest `q` the API takes, in characters. */
export const MAX_REDIRECT_QUERY = 200;

export function getRedirect(id: string): Promise<RedirectDto> {
  return unwrap(api.GET("/api/v1/redirects/{id}", { params: { path: { id } } }));
}

/**
 * The redirect stored for exactly `source`, else `null`: searched with `q` = the source
 * (its first 200 characters, a substring of it all the same), page by page until found.
 */
export async function findRedirect(source: string): Promise<RedirectDto | null> {
  const q = Array.from(source.trim()).slice(0, MAX_REDIRECT_QUERY).join("");
  if (q === "") return null;
  let cursor: string | undefined;
  for (;;) {
    const page = await listRedirects({ q, limit: MAX_PAGE, ...(cursor ? { cursor } : {}) });
    const hit = page.items.find((r) => r.source === source);
    if (hit) return hit;
    if (!page.next_cursor) return null;
    cursor = page.next_cursor;
  }
}

/** Creates a redirect, or changes the one with the same source. */
export function upsertRedirect(body: SchemaRedirectInput): Promise<RedirectDto> {
  return unwrap(api.POST("/api/v1/redirects", { body }));
}

export function patchRedirect(id: string, body: SchemaRedirectPatch): Promise<RedirectDto> {
  return unwrap(api.PATCH("/api/v1/redirects/{id}", { params: { path: { id } }, body }));
}

export async function deleteRedirect(id: string): Promise<void> {
  await unwrap(api.DELETE("/api/v1/redirects/{id}", { params: { path: { id } } }));
}

/** `source,target,code` rows (RFC 4180). */
export function importRedirects(csv: string): Promise<SchemaImportReportDto> {
  return unwrap(
    api.POST("/api/v1/redirects/import", {
      body: csv,
      bodySerializer: (b) => b,
      headers: { "Content-Type": "text/csv" },
    }),
  );
}
