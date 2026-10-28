import { describe, expect, it } from "vitest";
import type { TermDto } from "@/api/taxonomies";
import { flatten } from "@/components/kit/tree-model";
import {
  childCount,
  descendants,
  filterTerms,
  parentChoices,
  parentFilter,
  toTree,
  withParent,
} from "./terms-model";

const term = (id: string, name: string, parent_id: string | null = null): TermDto => ({
  id,
  name,
  slug: name.toLowerCase().replace(/\s+/g, "-"),
  description: "",
  taxonomy: "category",
  parent_id,
});

/** Name and depth of each row, depth-first, as the tree shows them. */
const rows = (terms: TermDto[]) =>
  flatten(toTree(terms, "en")).map((n) => `${"  ".repeat(n.depth)}${n.data.name}`);

describe("terms_model_test", () => {
  it("siblings_sort_by_name_whatever_the_server_order", () => {
    const terms = [
      term("c", "cherry"),
      term("b", "Banana"),
      term("i10", "Item 10", "c"),
      term("i2", "Item 2", "c"),
      term("a", "apple"),
    ];
    // Case does not matter; numbers inside names sort as numbers.
    expect(rows(terms)).toEqual(["apple", "Banana", "cherry", "  Item 2", "  Item 10"]);
  });

  it("a_term_whose_parent_is_missing_becomes_a_root", () => {
    const nodes = toTree([term("a", "Alpha", "gone"), term("b", "Beta", "a")], "en");
    expect(nodes.find((n) => n.id === "a")?.parentId).toBeNull();
    expect(nodes.find((n) => n.id === "b")?.parentId).toBe("a");
    expect(rows([term("a", "Alpha", "gone"), term("b", "Beta", "a")])).toEqual(["Alpha", "  Beta"]);
  });

  it("a_cycle_in_the_data_is_cut_never_lost", () => {
    // Core refuses cycles, but two concurrent moves can still form one (P2).
    const terms = [term("x", "Xeno", "y"), term("y", "Yarrow", "x"), term("r", "Root")];
    const shown = rows(terms);
    expect(shown).toHaveLength(3);
    expect(shown[0]).toBe("Root");
    expect(shown.slice(1)).toEqual(["Xeno", "  Yarrow"]);
  });

  it("descendants_are_every_term_below_never_the_term_itself", () => {
    const terms = [
      term("a", "A"),
      term("b", "B", "a"),
      term("c", "C", "b"),
      term("d", "D", "a"),
      term("e", "E"),
    ];
    expect([...descendants(terms, "a")].sort()).toEqual(["b", "c", "d"]);
    expect([...descendants(terms, "b")]).toEqual(["c"]);
    expect(descendants(terms, "e").size).toBe(0);
    expect(descendants(terms, "unknown").size).toBe(0);
    expect(childCount(terms, "a")).toBe(2);
    expect(childCount(terms, "c")).toBe(0);
  });

  it("parent_choices_leave_out_the_term_and_its_subtree", () => {
    const terms = [term("a", "A"), term("b", "B", "a"), term("c", "C", "b"), term("e", "E")];
    // Moving B: neither B nor C may become its parent.
    expect(parentChoices(terms, "b", "en").map((c) => [c.term.id, c.depth])).toEqual([
      ["a", 0],
      ["e", 0],
    ]);
    // A new term: every term, in tree order, with its path for the label.
    const all = parentChoices(terms, null, "en");
    expect(all.map((c) => c.term.id)).toEqual(["a", "b", "c", "e"]);
    expect(all.find((c) => c.term.id === "c")?.path).toEqual(["A", "B", "C"]);
  });

  it("with_parent_moves_one_term_and_keeps_the_rest", () => {
    const terms = [term("a", "A"), term("b", "B")];
    const next = withParent(terms, "b", "a");
    expect(next.find((t) => t.id === "b")?.parent_id).toBe("a");
    expect(next.find((t) => t.id === "a")).toBe(terms[0]);
    expect(terms[1]?.parent_id).toBeNull();
    expect(withParent(next, "b", null).find((t) => t.id === "b")?.parent_id).toBeNull();
  });

  it("filter_matches_every_word_in_name_slug_or_description", () => {
    const terms = [
      { ...term("p", "Pâtisserie"), description: "Cakes and tarts" },
      term("b", "Bread"),
      { ...term("s", "Sourdough"), slug: "levain" },
    ];
    const names = (q: string) => filterTerms(terms, q, "en").map((x) => x.name);
    expect(names("")).toEqual(["Bread", "Pâtisserie", "Sourdough"]);
    // Accents and case aside; every word must match somewhere.
    expect(names("PATISS")).toEqual(["Pâtisserie"]);
    expect(names("cakes tarts")).toEqual(["Pâtisserie"]);
    expect(names("cakes bread")).toEqual([]);
    expect(names("levain")).toEqual(["Sourdough"]);
  });
});

describe("parentFilter", () => {
  it("parent_filter_matches_names_and_slugs_not_ids", () => {
    const id = "3f2a9c1e-7b4d-4e8a-9c0f-1a2b3c4d5e6f";
    expect(parentFilter(id, "fru", ["Fruit", "fruit"])).toBeGreaterThan(0);
    expect(parentFilter(id, "apples-slug", ["Apples", "apples-slug"])).toBeGreaterThan(0);
    // Pieces of the id find nothing.
    expect(parentFilter(id, "3f2a", ["Fruit", "fruit"])).toBe(0);
    expect(parentFilter(id, "7b4d", ["Fruit", "fruit"])).toBe(0);
  });
});
