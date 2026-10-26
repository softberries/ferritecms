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

export function patchTerm(id: string, body: SchemaTermPatch): Promise<SchemaTermDto> {
  return unwrap(api.PATCH("/api/v1/terms/{id}", { params: { path: { id } }, body }));
}

export async function deleteTerm(id: string): Promise<void> {
  await unwrap(api.DELETE("/api/v1/terms/{id}", { params: { path: { id } } }));
}
