import { api, unwrap } from "./client";
import type { SchemaMetaDto } from "./schema";

/** `GET /meta`: the API version, the server's version and `site.url` (no credentials). */
export function meta(): Promise<SchemaMetaDto> {
  return unwrap(api.GET("/api/v1/meta"));
}
