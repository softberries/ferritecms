import { describe, expect, it } from "vitest";
import type { MenuDto } from "@/api/menus";
import { flatten, type TreeNode } from "@/components/kit/tree-model";
import {
  MAX_MENU_ITEMS,
  MENU_MAX_DEPTH,
  type MenuEntry,
  checkMenu,
  fromDto,
  menuNameProblem,
  nameFromMenuLabel,
  newNode,
  removeEntry,
  restoreEntry,
  toInput,
  validMenuUrl,
} from "./menu-model";

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

/** The server's tree order: roots first, each followed by its children. */
const DTO: MenuDto = {
  name: "primary",
  label: "Primary",
  locale: "en",
  items: [
    { id: "a", kind: "url", url: "/", label: "Home", parent_id: null, position: 0 },
    { id: "b", kind: "item", target_id: "t-about", label: "", parent_id: null, position: 1 },
    { id: "c", kind: "term", target_id: "t-news", label: "News", parent_id: "b", position: 0 },
    { id: "d", kind: "url", url: "https://x.test", label: "X", parent_id: "b", position: 1 },
    { id: "e", kind: "item", target_id: "t-contact", label: "Contact", position: 2 },
  ],
};

const entry = (label: string): MenuEntry => ({ kind: "url", targetId: null, url: "/", label });
const shape = (nodes: TreeNode<MenuEntry>[]) =>
  flatten(nodes).map((n) => `${n.depth}:${n.data.label || (n.data.targetId ?? "")}`);

describe("menu_model_test", () => {
  it("round_trip", () => {
    const nodes = fromDto(DTO);
    expect(shape(nodes)).toEqual(["0:Home", "0:t-about", "1:News", "1:X", "0:Contact"]);
    const input = toInput(nodes, "Primary");
    expect(input.label).toBe("Primary");
    // Every member the server reads, normalised (absent ones are explicit nulls).
    expect(input.items).toEqual([
      {
        id: "a",
        kind: "url",
        target_id: null,
        url: "/",
        label: "Home",
        parent_id: null,
        position: 0,
      },
      {
        id: "b",
        kind: "item",
        target_id: "t-about",
        url: null,
        label: "",
        parent_id: null,
        position: 1,
      },
      {
        id: "c",
        kind: "term",
        target_id: "t-news",
        url: null,
        label: "News",
        parent_id: "b",
        position: 0,
      },
      {
        id: "d",
        kind: "url",
        target_id: null,
        url: "https://x.test",
        label: "X",
        parent_id: "b",
        position: 1,
      },
      {
        id: "e",
        kind: "item",
        target_id: "t-contact",
        url: null,
        label: "Contact",
        parent_id: null,
        position: 2,
      },
    ]);
    // Reading what was sent gives the same tree.
    expect(
      fromDto({ name: "primary", label: "Primary", locale: "en", items: input.items ?? [] }),
    ).toEqual(nodes);
  });

  it("siblings_follow_position_whatever_the_array_order", () => {
    const nodes = fromDto({
      name: "m",
      label: "",
      locale: "en",
      items: [
        { id: "z", kind: "url", url: "/z", label: "Z", position: 2 },
        { id: "y", kind: "url", url: "/y", label: "Y", position: 0 },
        { id: "x", kind: "url", url: "/x", label: "X", position: 1 },
      ],
    });
    expect(shape(nodes)).toEqual(["0:Y", "0:X", "0:Z"]);
  });

  it("positions_are_sibling_indexes_after_moves", () => {
    // A gap and a duplicate in the stored positions come out as 0, 1, 2 per parent.
    const nodes = fromDto({
      name: "m",
      label: "",
      locale: "en",
      items: [
        { id: "p", kind: "url", url: "/p", label: "P", position: 5 },
        { id: "q", kind: "url", url: "/q", label: "Q", position: 9 },
        { id: "q1", kind: "url", url: "/q1", label: "Q1", parent_id: "q", position: 3 },
        { id: "q2", kind: "url", url: "/q2", label: "Q2", parent_id: "q", position: 3 },
      ],
    });
    const items = toInput(nodes, "").items ?? [];
    expect(items.map((i) => [i.id, i.parent_id, i.position])).toEqual([
      ["p", null, 0],
      ["q", null, 1],
      ["q1", "q", 0],
      ["q2", "q", 1],
    ]);
  });

  it("new_ids", () => {
    const a = newNode(entry("A"));
    const b = newNode(entry("B"));
    expect(a.id).toMatch(UUID);
    expect(b.id).toMatch(UUID);
    expect(a.id).not.toBe(b.id);
    expect(a.parentId).toBeNull();
    const items = toInput([...fromDto(DTO), a], "Primary").items ?? [];
    expect(items.at(-1)).toMatchObject({ id: a.id, parent_id: null, position: 3 });
  });

  it("limits", () => {
    // Core's cap: at most 500 entries.
    const many = Array.from({ length: MAX_MENU_ITEMS }, (_, i) => newNode(entry(`L${i}`)));
    expect(checkMenu(many)).toEqual([]);
    expect(checkMenu([...many, newNode(entry("one more"))])).toEqual([
      { kind: "tooMany", count: MAX_MENU_ITEMS + 1 },
    ]);
    // Depth: a chain one level past the deepest the site renders.
    const chain: TreeNode<MenuEntry>[] = [];
    for (let depth = 0; depth <= MENU_MAX_DEPTH + 1; depth++) {
      chain.push({
        id: `n${depth}`,
        parentId: depth === 0 ? null : `n${depth - 1}`,
        data: entry(""),
      });
    }
    expect(checkMenu(chain.slice(0, MENU_MAX_DEPTH + 1))).toEqual([]);
    expect(checkMenu(chain)).toEqual([{ kind: "tooDeep", id: `n${MENU_MAX_DEPTH + 1}` }]);
    // Labels and custom URLs as core checks them.
    const long = newNode(entry("x".repeat(201)));
    const bad = newNode({ kind: "url", targetId: null, url: "javascript:alert(1)", label: "" });
    expect(checkMenu([long, bad])).toEqual([
      { kind: "labelTooLong", id: long.id },
      { kind: "badUrl", id: bad.id },
    ]);
  });

  it("remove_promotes_children_and_restore_puts_them_back", () => {
    const nodes = fromDto(DTO);
    const { nodes: after, removed } = removeEntry(nodes, "b");
    // News and X take b's place among the roots.
    expect(shape(after)).toEqual(["0:Home", "0:News", "0:X", "0:Contact"]);
    // The same tree as before (the array order may differ; siblings keep theirs).
    expect(toInput(restoreEntry(after, removed), "")).toEqual(toInput(nodes, ""));
    // After another edit the entry still comes back, under its parent, with its children.
    const edited = after.filter((n) => n.id !== "a");
    expect(shape(restoreEntry(edited, removed))).toEqual([
      "0:t-about",
      "1:News",
      "1:X",
      "0:Contact",
    ]);
    // An unknown id changes nothing.
    expect(removeEntry(nodes, "nope").nodes).toBe(nodes);
  });

  it("custom_urls_follow_core", () => {
    // `is_allowed_url` in crates/ferrite-core/src/menu.rs.
    for (const ok of ["https://a.b/c", "HTTP://a.b", "/x", "/"])
      expect(validMenuUrl(ok)).toBe(true);
    for (const bad of [
      "",
      "//evil.com",
      "javascript:alert(1)",
      "ftp://a",
      "x",
      "http://",
      "/a b",
      "/\\evil",
      "mailto:a@b.c",
      `/${"a".repeat(2048)}`,
    ]) {
      expect(validMenuUrl(bad)).toBe(false);
    }
  });

  it("menu_names_follow_core", () => {
    // `slug::validate` and "must equal its normalised form" in core's `MenuService::put`.
    expect(menuNameProblem("primary")).toBeNull();
    expect(menuNameProblem("social_links-2")).toBeNull();
    expect(menuNameProblem("")).toBe("empty");
    expect(menuNameProblem("Primary")).toBe("form");
    expect(menuNameProblem(" primary")).toBe("form");
    expect(menuNameProblem("a/b")).toBe("chars");
    expect(menuNameProblem("a b")).toBe("chars");
    expect(menuNameProblem("..")).toBe("chars");
    expect(menuNameProblem("x".repeat(201))).toBe("tooLong");
    expect(nameFromMenuLabel("Social links!")).toBe("social-links");
    expect(nameFromMenuLabel("  Über Uns ")).toBe("uber-uns");
    expect(nameFromMenuLabel("!!!")).toBe("");
  });
});
