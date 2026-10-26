import { api, unwrap } from "./client";
import type { SchemaMenuDto, SchemaMenuInput, SchemaMenuList } from "./schema";

export type MenuDto = SchemaMenuDto;

export function listMenus(): Promise<SchemaMenuList> {
  return unwrap(api.GET("/api/v1/menus"));
}

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
