import { api, unwrap } from "./client";
import type { SchemaTermDto, SchemaTermPatch } from "./schema";

export { createTerm } from "./taxonomies";

/** `GET /taxonomies/{t}/terms`: every term of the taxonomy. */
export async function listTerms(taxonomy: string): Promise<SchemaTermDto[]> {
  const list = await unwrap(
    api.GET("/api/v1/taxonomies/{taxonomy}/terms", { params: { path: { taxonomy } } }),
  );
  return list.items;
}

/** `GET /terms/{id}`: the term as stored now (the stale guard's re-read). */
export function getTerm(id: string): Promise<SchemaTermDto> {
  return unwrap(api.GET("/api/v1/terms/{id}", { params: { path: { id } } }));
}

export function patchTerm(id: string, body: SchemaTermPatch): Promise<SchemaTermDto> {
  return unwrap(api.PATCH("/api/v1/terms/{id}", { params: { path: { id } }, body }));
}

/** `POST /terms/{id}/translations`: the term in `locale`, same name and slug (M6-1). */
export function translateTerm(id: string, locale: string): Promise<SchemaTermDto> {
  return unwrap(
    api.POST("/api/v1/terms/{id}/translations", { params: { path: { id } }, body: { locale } }),
  );
}

export async function deleteTerm(id: string): Promise<void> {
  await unwrap(api.DELETE("/api/v1/terms/{id}", { params: { path: { id } } }));
}
