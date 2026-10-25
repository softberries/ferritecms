import { queryOptions } from "@tanstack/react-query";
import { api, unwrap } from "./client";
import { qk } from "./keys";
import type { SchemaSettingsDto } from "./schema";

export type SettingsDto = SchemaSettingsDto;

/** `GET /settings`: every site setting (`null` = the default). */
export function getSettings(): Promise<SettingsDto> {
  return unwrap(api.GET("/api/v1/settings"));
}

export const settingsQuery = queryOptions({
  queryKey: qk.settings,
  queryFn: getSettings,
  staleTime: 60_000,
});
