import { queryOptions } from "@tanstack/react-query";
import { api, unwrap } from "./client";
import { qk } from "./keys";
import type {
  SchemaPermissionReportDto,
  SchemaPluginDto,
  SchemaPluginInspectUpload,
  SchemaPluginMetricsDto,
  SchemaPluginSettingsDto,
  SchemaPluginUpload,
  SchemaRetainedPluginDto,
  SchemaSettingsPatch,
} from "./schema";

export type PluginDto = SchemaPluginDto;
export type PluginMetricsDto = SchemaPluginMetricsDto;
export type PluginSettingsDto = SchemaPluginSettingsDto;
export type PermissionReportDto = SchemaPermissionReportDto;
export type RetainedPluginDto = SchemaRetainedPluginDto;

/** `GET /plugins` (`manage_plugins`): every installed plugin, by name (one page). */
export async function listPlugins(): Promise<PluginDto[]> {
  const list = await unwrap(api.GET("/api/v1/plugins"));
  return list.items;
}

/** `GET /plugins/{name}/metrics`: failures per hook and hour over 24 h, every node's. */
export function getPluginMetrics(name: string): Promise<PluginMetricsDto> {
  return unwrap(api.GET("/api/v1/plugins/{name}/metrics", { params: { path: { name } } }));
}

export const pluginsQuery = queryOptions({
  queryKey: qk.plugins,
  queryFn: listPlugins,
  staleTime: 30_000,
});

export function pluginMetricsQuery(name: string) {
  return queryOptions({
    queryKey: qk.pluginMetrics(name),
    queryFn: () => getPluginMetrics(name),
    staleTime: 30_000,
  });
}

export function getPlugin(name: string): Promise<PluginDto> {
  return unwrap(api.GET("/api/v1/plugins/{name}", { params: { path: { name } } }));
}

export function pluginQuery(name: string) {
  return queryOptions({
    queryKey: qk.plugin(name),
    queryFn: () => getPlugin(name),
    staleTime: 30_000,
  });
}

/** The two files of a plugin upload: `ferrite-plugin.toml` and its component. */
export interface PluginFiles {
  manifest: Blob;
  component: Blob;
}

function pluginForm(files: PluginFiles, approve?: string): FormData {
  const form = new FormData();
  form.append("manifest", files.manifest);
  form.append("component", files.component);
  if (approve !== undefined) form.append("approve", approve);
  return form;
}

/** `POST /plugins/inspect` (multipart): the permission report; nothing is stored. */
export function inspectPlugin(files: PluginFiles): Promise<PermissionReportDto> {
  return unwrap(
    api.POST("/api/v1/plugins/inspect", {
      // The browser sets the multipart `Content-Type` with its boundary.
      body: pluginForm(files) as unknown as SchemaPluginInspectUpload,
      bodySerializer: (b) => b as unknown as FormData,
    }),
  );
}

/**
 * `POST /plugins` (multipart): installs exactly what was inspected, `approve` being the
 * report's `fingerprint`. Files changed meanwhile: `422` field `approve`, with the new
 * `report` in the problem. A new plugin arrives disabled.
 */
export function installPlugin(files: PluginFiles & { approve: string }): Promise<PluginDto> {
  return unwrap(
    api.POST("/api/v1/plugins", {
      body: pluginForm(files, files.approve) as unknown as SchemaPluginUpload,
      bodySerializer: (b) => b as unknown as FormData,
    }),
  );
}

export function enablePlugin(name: string): Promise<PluginDto> {
  return unwrap(api.POST("/api/v1/plugins/{name}/enable", { params: { path: { name } } }));
}

export function disablePlugin(name: string): Promise<PluginDto> {
  return unwrap(api.POST("/api/v1/plugins/{name}/disable", { params: { path: { name } } }));
}

/**
 * `DELETE /plugins/{name}`: keeps its data 30 days, or erases it at once with
 * `purge: true` (two separate choices in the UI, H-P3).
 */
export async function deletePlugin(name: string, options: { purge?: boolean } = {}): Promise<void> {
  await unwrap(
    api.DELETE("/api/v1/plugins/{name}", {
      params: { path: { name }, ...(options.purge ? { query: { purge: true } } : {}) },
    }),
  );
}

/** `GET /plugins/retained`: deleted plugins whose data is kept, by name. */
export async function listRetainedPlugins(): Promise<RetainedPluginDto[]> {
  const list = await unwrap(api.GET("/api/v1/plugins/retained"));
  return list.items;
}

export const retainedPluginsQuery = queryOptions({
  queryKey: qk.pluginsRetained,
  queryFn: listRetainedPlugins,
  staleTime: 30_000,
});

/** `POST /plugins/{name}/purge`: erases a deleted plugin's kept data now. */
export async function purgePlugin(name: string): Promise<void> {
  await unwrap(api.POST("/api/v1/plugins/{name}/purge", { params: { path: { name } } }));
}

/** `GET /plugins/{name}/settings`: schema, non-secret values, which secrets are set, `invalid`. */
export function getPluginSettings(name: string): Promise<PluginSettingsDto> {
  return unwrap(api.GET("/api/v1/plugins/{name}/settings", { params: { path: { name } } }));
}

export function pluginSettingsQuery(name: string) {
  return queryOptions({
    queryKey: qk.pluginSettings(name),
    queryFn: () => getPluginSettings(name),
    staleTime: 30_000,
  });
}

/**
 * `PATCH /plugins/{name}/settings`: only the given keys. A secret key absent is kept, a
 * string sets it, `null` deletes it.
 */
export function patchPluginSettings(
  name: string,
  patch: Record<string, unknown>,
): Promise<PluginSettingsDto> {
  return unwrap(
    api.PATCH("/api/v1/plugins/{name}/settings", {
      params: { path: { name } },
      // `SettingsPatch` is an open object the generator types as `Record<string, never>`.
      body: patch as SchemaSettingsPatch,
    }),
  );
}
