import { queryOptions } from "@tanstack/react-query";
import { api, unwrap } from "./client";
import { qk } from "./keys";
import type { SchemaMetaDto } from "./schema";

/** `GET /meta`: the API version, the server's version and `site.url` (no credentials). */
export function meta(): Promise<SchemaMetaDto> {
  return unwrap(api.GET("/api/v1/meta"));
}

/** The server's identity does not change while the page is open. */
export const metaQuery = queryOptions({ queryKey: qk.meta, queryFn: meta, staleTime: Infinity });
