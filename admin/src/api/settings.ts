import { queryOptions } from "@tanstack/react-query";
import { api, unwrap } from "./client";
import { qk } from "./keys";
import type { SchemaSettingsDto } from "./schema";

export type SettingsDto = SchemaSettingsDto;

/** `GET /settings`: every site setting (`null` = the default). */
export function getSettings(): Promise<SettingsDto> {
  return unwrap(api.GET("/api/v1/settings"));
}

/**
 * `PATCH /settings` (`manage_settings`): only the given keys, `null` resets one; answers
 * every setting. Last write wins: the screens save through `staleGuard`.
 */
export function patchSettings(patch: SettingsDto): Promise<SettingsDto> {
  return unwrap(api.PATCH("/api/v1/settings", { body: patch }));
}

export const settingsQuery = queryOptions({
  queryKey: qk.settings,
  queryFn: getSettings,
  staleTime: 60_000,
});
