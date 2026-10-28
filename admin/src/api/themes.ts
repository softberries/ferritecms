import { queryOptions } from "@tanstack/react-query";
import { api, unwrap } from "./client";
import { qk } from "./keys";
import type {
  SchemaSettingDefDto,
  SchemaSettingsPatch,
  SchemaThemeDto,
  SchemaThemeSettingsDto,
} from "./schema";

export type ThemeDto = SchemaThemeDto;
export type ThemeSettingsDto = SchemaThemeSettingsDto;
/** One declared setting of a theme or a plugin (`SettingsForm` renders it). */
export type SettingDefDto = SchemaSettingDefDto;

/** `GET /themes` (`manage_themes`): installed and embedded themes; the active one has `nodes`. */
export async function listThemes(): Promise<ThemeDto[]> {
  const list = await unwrap(api.GET("/api/v1/themes"));
  return list.items;
}

export const themesQuery = queryOptions({
  queryKey: qk.themes,
  queryFn: listThemes,
  staleTime: 30_000,
});

/**
 * `POST /themes`: a canonical ustar bundle (`application/x-tar`), replacing a theme of
 * that name (an active one goes live at once). The upload screen may use an XHR for
 * progress instead; this is the plain request.
 */
export function installTheme(
  tar: Uint8Array | Blob,
  options: { activate?: boolean } = {},
): Promise<ThemeDto> {
  const body = tar instanceof Blob ? tar : new Blob([tar as Uint8Array<ArrayBuffer>]);
  return unwrap(
    api.POST("/api/v1/themes", {
      params: { query: { activate: options.activate ?? false } },
      // The generated type is the schema's `string` (binary); the body is the archive.
      body: body as unknown as string,
      bodySerializer: (b) => b,
      headers: { "Content-Type": "application/x-tar" },
    }),
  );
}

/** `POST /themes/{name}/activate`: `422` when it does not load (nothing changes). */
export function activateTheme(name: string): Promise<ThemeDto> {
  return unwrap(api.POST("/api/v1/themes/{name}/activate", { params: { path: { name } } }));
}

/** `GET /themes/{name}/settings`: the schema and every value as templates see it. */
export function getThemeSettings(name: string): Promise<ThemeSettingsDto> {
  return unwrap(api.GET("/api/v1/themes/{name}/settings", { params: { path: { name } } }));
}

export function themeSettingsQuery(name: string) {
  return queryOptions({
    queryKey: qk.themeSettings(name),
    queryFn: () => getThemeSettings(name),
    staleTime: 30_000,
  });
}

/** `PATCH /themes/{name}/settings`: only the given keys; `null` resets one. */
export function patchThemeSettings(
  name: string,
  patch: Record<string, unknown>,
): Promise<ThemeSettingsDto> {
  return unwrap(
    api.PATCH("/api/v1/themes/{name}/settings", {
      params: { path: { name } },
      // `SettingsPatch` is an open object the generator types as `Record<string, never>`.
      body: patch as SchemaSettingsPatch,
    }),
  );
}
