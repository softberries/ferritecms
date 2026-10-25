import { queryOptions } from "@tanstack/react-query";
import { api, unwrap } from "./client";
import { qk } from "./keys";
import type { SchemaPluginDto, SchemaPluginMetricsDto } from "./schema";

export type PluginDto = SchemaPluginDto;
export type PluginMetricsDto = SchemaPluginMetricsDto;

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
