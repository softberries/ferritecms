// Menus per locale (M6-1 Task 16): the lookups behind `menu-locales.tsx`.

import type { SchemaMenuItemDto, SchemaMenuSummary } from "@/api/schema";

/** The menu that translates `name` into `tag`, if any. */
export function translationOf(
  menus: readonly SchemaMenuSummary[],
  name: string,
  tag: string,
): SchemaMenuSummary | undefined {
  return menus.find((m) => m.translation_of === name && m.locale === tag);
}

/** Entries with fresh ids (entry ids are unique across menus), parents mapped. */
export function copyEntries(items: readonly SchemaMenuItemDto[]): SchemaMenuItemDto[] {
  const ids = new Map(items.map((i) => [i.id, crypto.randomUUID()]));
  return items.map((i) => ({
    ...i,
    id: ids.get(i.id) ?? crypto.randomUUID(),
    parent_id: i.parent_id ? (ids.get(i.parent_id) ?? null) : null,
  }));
}
