import { keepPreviousData, queryOptions } from "@tanstack/react-query";
import { api, unwrap } from "./client";
import { qk } from "./keys";
import type {
  SchemaPermissionReportDto,
  SchemaPluginDto,
  SchemaPluginMetricsDto,
  SchemaPluginSettingsDto,
  SchemaPublisherKeyDto,
  SchemaPublisherKeyList,
  SchemaRegistryDto,
  SchemaRegistryPluginDto,
  SchemaRetainedPluginDto,
  SchemaSettingsPatch,
  SchemaTrustDto,
} from "./schema";

export type PluginDto = SchemaPluginDto;
export type PluginMetricsDto = SchemaPluginMetricsDto;
export type PluginSettingsDto = SchemaPluginSettingsDto;
export type PermissionReportDto = SchemaPermissionReportDto;
export type RetainedPluginDto = SchemaRetainedPluginDto;
export type TrustDto = SchemaTrustDto;
export type RegistryDto = SchemaRegistryDto;
export type RegistryPluginDto = SchemaRegistryPluginDto;
export type PublisherKeyDto = SchemaPublisherKeyDto;
export type PublisherKeyList = SchemaPublisherKeyList;

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

/**
 * The files of a plugin upload: `ferrite-plugin.toml`, its component and, for a signed
 * plugin, `ferrite-plugin.minisig` and (for a key the site does not know yet)
 * `ferrite-plugin.pub` (M6-2).
 */
export interface PluginFiles {
  manifest: Blob;
  component: Blob;
  signature?: Blob | null;
  publisherKey?: Blob | null;
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

/** `GET /plugins/registry?q=`: the configured registry and its cached plugins (no fetch). */
export function getRegistry(q?: string): Promise<RegistryDto> {
  return unwrap(api.GET("/api/v1/plugins/registry", { params: { query: q ? { q } : {} } }));
}

export function registryQuery(q = "") {
  return queryOptions({
    queryKey: qk.pluginRegistry(q),
    queryFn: () => getRegistry(q || undefined),
    staleTime: 30_000,
    // A new search keeps the last list (and the search box) until it answers.
    placeholderData: keepPreviousData,
  });
}

/** `POST /plugins/registry/refresh`: fetches and verifies the index now (1 per minute). */
export function refreshRegistry(): Promise<RegistryDto> {
  return unwrap(api.POST("/api/v1/plugins/registry/refresh"));
}

/** `POST /plugins/registry/inspect`: a registry version's permission report. */
export function inspectRegistryPlugin(
  name: string,
  version?: string,
): Promise<PermissionReportDto> {
  return unwrap(
    api.POST("/api/v1/plugins/registry/inspect", {
      body: { name, ...(version ? { version } : {}) },
    }),
  );
}

/** `POST /plugins/registry/install`: installs it when `approve` is that report's fingerprint. */
export function installRegistryPlugin(
  name: string,
  version: string,
  approve: string,
): Promise<PluginDto> {
  return unwrap(api.POST("/api/v1/plugins/registry/install", { body: { name, version, approve } }));
}

/** `GET /plugins/publishers`: the site's keys, the registry's keys and the revocations. */
export function listPublishers(): Promise<PublisherKeyList> {
  return unwrap(api.GET("/api/v1/plugins/publishers"));
}

export const publishersQuery = queryOptions({
  queryKey: qk.pluginPublishers,
  queryFn: listPublishers,
  staleTime: 30_000,
});

/** `POST /plugins/publishers/{key_id}/revoke`. */
export function revokePublisher(
  keyId: string,
  reason: string,
  compromised: boolean,
): Promise<PublisherKeyDto> {
  return unwrap(
    api.POST("/api/v1/plugins/publishers/{key_id}/revoke", {
      params: { path: { key_id: keyId } },
      body: { reason, compromised },
    }),
  );
}
