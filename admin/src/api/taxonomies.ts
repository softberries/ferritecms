import { queryOptions } from "@tanstack/react-query";
import { api, unwrap } from "./client";
import { qk } from "./keys";
import type { SchemaTaxonomyDto, SchemaTermDto, SchemaTermInput } from "./schema";

export type TaxonomyDto = SchemaTaxonomyDto;
export type TermDto = SchemaTermDto;

/** `GET /taxonomies`: every taxonomy (one page holds them all). */
export const taxonomiesQuery = queryOptions({
  queryKey: qk.taxonomies,
  queryFn: async () => (await unwrap(api.GET("/api/v1/taxonomies"))).items,
  staleTime: 5 * 60_000,
});

/** `GET /taxonomies/{t}/terms`: every term of the taxonomy (one page holds them all). */
export function termsQuery(taxonomy: string) {
  return queryOptions({
    queryKey: qk.terms(taxonomy),
    queryFn: async () =>
      (
        await unwrap(
          api.GET("/api/v1/taxonomies/{taxonomy}/terms", { params: { path: { taxonomy } } }),
        )
      ).items,
    staleTime: 60_000,
  });
}

/** `POST /taxonomies/{t}/terms`: a new term (needs `manage_terms`; the server checks). */
export async function createTerm(taxonomy: string, input: SchemaTermInput): Promise<SchemaTermDto> {
  return unwrap(
    api.POST("/api/v1/taxonomies/{taxonomy}/terms", {
      params: { path: { taxonomy } },
      body: input,
    }),
  );
}
