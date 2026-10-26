// A menu as the editor holds it: the `SortableTree`'s nodes over the entries of
// `MenuDto`, and back to the `PUT /menus/{name}` body. Siblings are ordered by the
// array (`tree-model.ts`); `position` is only what the server stores. The limits and
// rules here are core's (`crates/ferrite-core/src/menu.rs`, `slug.rs`), checked before
// the request so the editor can say what is wrong; the server checks again.

import type { MenuDto } from "@/api/menus";
import type { SchemaMenuInput, SchemaMenuItemDto, SchemaMenuItemKindDto } from "@/api/schema";
import { flatten, type TreeNode } from "@/components/kit/tree-model";

export type MenuItemKind = SchemaMenuItemKindDto;

export interface MenuEntry {
  kind: MenuItemKind;
  /** The content item (`item`) or term (`term`); `null` for a custom link. */
  targetId: string | null;
  /** Only for a custom link (`url`). */
  url: string | null;
  /** Empty: the site shows the target's title (or the link's address). */
  label: string;
}

export type MenuNode = TreeNode<MenuEntry>;

/** Most entries one menu holds (`MAX_MENU_ITEMS` in core's `menu.rs`). */
export const MAX_MENU_ITEMS = 500;
/** Longest menu label and entry label, in characters (`MAX_LABEL_LEN`). */
export const MAX_MENU_LABEL = 200;
/** Longest custom link address, in characters (`MAX_URL_LEN`). */
export const MAX_MENU_URL = 2048;
/** Longest menu name (`MAX_LEN` in core's `slug.rs`). */
export const MAX_MENU_NAME = 200;
/**
 * The deepest level an entry may reach (0 = top level). Core's `menu.rs` sets no depth
 * limit of its own (only the 500 entries); the public site renders 16 levels
 * (`MAX_MENU_DEPTH` in `crates/ferrite-http/src/app/bridge.rs`) and drops deeper
 * entries, so the editor stops at the 16th level.
 */
export const MENU_MAX_DEPTH = 15;

/** The server's entries as tree nodes, siblings in `position` order (then id, as core). */
export function fromDto(menu: MenuDto): MenuNode[] {
  return [...menu.items]
    .sort(
      (a, b) => (a.position ?? 0) - (b.position ?? 0) || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0),
    )
    .map((item) => ({
      id: item.id,
      parentId: item.parent_id ?? null,
      data: {
        kind: item.kind,
        targetId: item.kind === "url" ? null : (item.target_id ?? null),
        url: item.kind === "url" ? (item.url ?? null) : null,
        label: item.label ?? "",
      },
    }));
}

/**
 * The `PUT` body: every entry in tree order, `parent_id` from the tree and `position` its
 * index among its siblings.
 */
export function toInput(nodes: MenuNode[], label: string): SchemaMenuInput {
  const items: SchemaMenuItemDto[] = flatten(nodes).map((n) => ({
    id: n.id,
    kind: n.data.kind,
    target_id: n.data.targetId,
    url: n.data.url,
    label: n.data.label,
    parent_id: n.parentId,
    position: n.index,
  }));
  return { label, items };
}

/** A new top-level entry with a fresh id (the client chooses entry ids). */
export function newNode(entry: MenuEntry): MenuNode {
  return { id: crypto.randomUUID(), parentId: null, data: entry };
}

export type MenuProblem =
  | { kind: "tooMany"; count: number }
  | { kind: "tooDeep"; id: string }
  | { kind: "labelTooLong"; id: string }
  | { kind: "badUrl"; id: string };

/** What core would refuse in `nodes` (and the levels the site would not show). */
export function checkMenu(nodes: MenuNode[]): MenuProblem[] {
  const problems: MenuProblem[] = [];
  if (nodes.length > MAX_MENU_ITEMS) problems.push({ kind: "tooMany", count: nodes.length });
  for (const n of flatten(nodes)) {
    if (n.depth > MENU_MAX_DEPTH) problems.push({ kind: "tooDeep", id: n.id });
    if (chars(n.data.label) > MAX_MENU_LABEL) problems.push({ kind: "labelTooLong", id: n.id });
    if (n.data.kind === "url" && !validMenuUrl(n.data.url ?? "")) {
      problems.push({ kind: "badUrl", id: n.id });
    }
  }
  return problems;
}

/** A removed entry, enough to put it back (`restoreEntry`). */
export interface Removed {
  node: MenuNode;
  /** Its place in the array (siblings follow the array). */
  at: number;
  /** Its direct children, which moved up to its place. */
  childIds: string[];
}

/** Removes `id`; its children move up into its place. An unknown id changes nothing. */
export function removeEntry(
  nodes: MenuNode[],
  id: string,
): { nodes: MenuNode[]; removed: Removed } {
  const at = nodes.findIndex((n) => n.id === id);
  const node = nodes[at];
  if (!node)
    return { nodes, removed: { node: { id, parentId: null, data: EMPTY }, at: -1, childIds: [] } };
  const children = nodes.filter((n) => n.parentId === id);
  const rest = nodes.filter((n) => n.id !== id && n.parentId !== id);
  const before = nodes.slice(0, at).filter((n) => n.parentId !== id).length;
  const moved = children.map((c) => ({ ...c, parentId: node.parentId }));
  return {
    nodes: [...rest.slice(0, before), ...moved, ...rest.slice(before)],
    removed: { node, at, childIds: children.map((c) => c.id) },
  };
}

/**
 * Puts a removed entry back: at its old place, under its old parent (the top level when
 * that is gone too), with its children that are still where the removal put them.
 */
export function restoreEntry(nodes: MenuNode[], removed: Removed): MenuNode[] {
  if (removed.at < 0 || nodes.some((n) => n.id === removed.node.id)) return nodes;
  const parentId =
    removed.node.parentId !== null && nodes.some((n) => n.id === removed.node.parentId)
      ? removed.node.parentId
      : null;
  const node = { ...removed.node, parentId };
  const children = new Set(removed.childIds);
  const back = nodes.map((n) =>
    children.has(n.id) && n.parentId === removed.node.parentId ? { ...n, parentId: node.id } : n,
  );
  // Back at its array index, or right before the first child that stayed, when earlier.
  const firstChild = back.findIndex((n) => n.parentId === node.id);
  const at = Math.min(removed.at, firstChild < 0 ? back.length : firstChild, back.length);
  return [...back.slice(0, at), node, ...back.slice(at)];
}

const EMPTY: MenuEntry = { kind: "url", targetId: null, url: null, label: "" };

/** Unicode scalar values, as Rust's `chars().count()`. */
const chars = (s: string) => [...s].length;

/**
 * A custom link core accepts (`is_allowed_url`): `http://` or `https://` (any case) with
 * something after it, or a path on this site (`/…`, not `//…`); no whitespace, control
 * characters or backslashes; at most 2048 characters.
 */
export function validMenuUrl(url: string): boolean {
  if (url === "" || chars(url) > MAX_MENU_URL) return false;
  // eslint-disable-next-line no-control-regex
  if (/[\s\u0000-\u001f\u007f-\u009f\\]/u.test(url)) return false;
  const lower = url.toLowerCase();
  for (const scheme of ["http://", "https://"]) {
    if (lower.startsWith(scheme)) return lower.length > scheme.length;
  }
  return url.startsWith("/") && !url.startsWith("//");
}

export type MenuNameProblem = "empty" | "tooLong" | "chars" | "form";

/**
 * What core says about a menu name (`slug::validate`, then "must equal its normalised
 * form" in `MenuService::put`): not empty, at most 200 characters, not `.` or `..`, no
 * whitespace, control or format characters or any of / ? # % < > " ' \ `, lower case
 * and without surrounding spaces.
 */
export function menuNameProblem(name: string): MenuNameProblem | null {
  const normal = name.trim().toLowerCase();
  if (normal === "") return "empty";
  if (chars(normal) > MAX_MENU_NAME) return "tooLong";
  if (normal === "." || normal === ".." || /[\s\p{Cc}\p{Cf}/?#%<>"'\\`]/u.test(normal)) {
    return "chars";
  }
  return normal === name ? null : "form";
}

/** A name for a menu labelled `label`: ASCII letters and digits, runs of others as `-`. */
export function nameFromMenuLabel(label: string): string {
  return label
    .normalize("NFD")
    .replace(/\p{M}/gu, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, MAX_MENU_NAME)
    .replace(/-+$/, "");
}
