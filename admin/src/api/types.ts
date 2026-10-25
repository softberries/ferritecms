import { queryOptions } from "@tanstack/react-query";
import { api, unwrap } from "./client";
import { qk } from "./keys";
import type { SchemaContentTypeDto } from "./schema";

export type ContentTypeDto = SchemaContentTypeDto;

/** `GET /types`: every content type (one page holds them all). */
export async function listTypes(): Promise<ContentTypeDto[]> {
  const list = await unwrap(api.GET("/api/v1/types"));
  return list.items;
}

/** Types change rarely (a plugin or an admin registers one); five minutes is fresh. */
export const typesQuery = queryOptions({
  queryKey: qk.types,
  queryFn: listTypes,
  staleTime: 5 * 60_000,
});
