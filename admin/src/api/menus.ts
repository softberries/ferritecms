import { queryOptions } from "@tanstack/react-query";
import { api, unwrap } from "./client";
import { qk } from "./keys";
import type { SchemaMenuDto, SchemaMenuInput, SchemaMenuList } from "./schema";

export type MenuDto = SchemaMenuDto;
export type MenuList = SchemaMenuList;

export function listMenus(): Promise<SchemaMenuList> {
  return unwrap(api.GET("/api/v1/menus"));
}

/** `GET /menus`: every menu and the active theme's menu locations. */
export const menusQuery = queryOptions({
  queryKey: qk.menus.list,
  queryFn: listMenus,
  staleTime: 30_000,
});

export function getMenu(name: string): Promise<MenuDto> {
  return unwrap(api.GET("/api/v1/menus/{name}", { params: { path: { name } } }));
}

/** Creates or replaces the menu; last write wins (no `If-Match`). */
export function putMenu(name: string, body: SchemaMenuInput): Promise<MenuDto> {
  return unwrap(api.PUT("/api/v1/menus/{name}", { params: { path: { name } }, body }));
}

export async function deleteMenu(name: string): Promise<void> {
  await unwrap(api.DELETE("/api/v1/menus/{name}", { params: { path: { name } } }));
}
