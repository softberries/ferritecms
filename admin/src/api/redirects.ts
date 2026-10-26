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

/** `GET /redirects`; a blank `q` is a `422`, so it is left out. */
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
