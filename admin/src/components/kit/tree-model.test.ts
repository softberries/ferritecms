import { describe, expect, it } from "vitest";
import { flatten, indent, moveNode, outdent, projectDrop, type TreeNode } from "./tree-model";

const node = (id: string, parentId: string | null = null): TreeNode<string> => ({
  id,
  parentId,
  data: id.toUpperCase(),
});

// a
// ├ b
// │ └ c
// └ d
// e
const TREE = [node("a"), node("b", "a"), node("e"), node("c", "b"), node("d", "a")];

const shape = (nodes: TreeNode<string>[]) =>
  flatten(nodes).map((n) => `${"-".repeat(n.depth)}${n.id}@${n.index}`);

describe("tree_model", () => {
  it("flatten_is_depth_first_in_sibling_order", () => {
    expect(shape(TREE)).toEqual(["a@0", "-b@0", "--c@0", "-d@1", "e@1"]);
    const flat = flatten(TREE);
    expect(flat.map((n) => n.parentId)).toEqual([null, "a", "b", "a", null]);
    // An orphan (its parent is gone) shows as a root rather than vanishing.
    expect(shape([node("x", "gone"), node("y")])).toEqual(["x@0", "y@1"]);
  });

  it("move_node_reparents_and_orders", () => {
    // e under a, between b and d.
    expect(shape(moveNode(TREE, "e", { parentId: "a", index: 1 }))).toEqual([
      "a@0",
      "-b@0",
      "--c@0",
      "-e@1",
      "-d@2",
    ]);
    // d to the root, first; its index past the end appends.
    expect(shape(moveNode(TREE, "d", { parentId: null, index: 0 }))).toEqual([
      "d@0",
      "a@1",
      "-b@0",
      "--c@0",
      "e@2",
    ]);
    expect(shape(moveNode(TREE, "a", { parentId: "e", index: 99 }))).toEqual([
      "e@0",
      "-a@0",
      "--b@0",
      "---c@0",
      "--d@1",
    ]);
  });

  it("move_node_into_own_subtree_is_refused", () => {
    expect(moveNode(TREE, "a", { parentId: "a", index: 0 })).toBe(TREE);
    expect(moveNode(TREE, "a", { parentId: "c", index: 0 })).toBe(TREE);
    expect(moveNode(TREE, "b", { parentId: "c", index: 0 })).toBe(TREE);
    // Unknown node or parent: unchanged too.
    expect(moveNode(TREE, "zz", { parentId: null, index: 0 })).toBe(TREE);
    expect(moveNode(TREE, "e", { parentId: "zz", index: 0 })).toBe(TREE);
  });

  it("indent_and_outdent", () => {
    // d becomes the last child of b (its previous sibling).
    expect(shape(indent(TREE, "d"))).toEqual(["a@0", "-b@0", "--c@0", "--d@1", "e@1"]);
    // b becomes the next sibling of a.
    expect(shape(outdent(TREE, "b"))).toEqual(["a@0", "-d@0", "b@1", "-c@0", "e@2"]);
  });

  it("indent_and_outdent_at_the_edges_are_no_ops", () => {
    // A first child has no previous sibling; a root has no parent.
    expect(indent(TREE, "b")).toBe(TREE);
    expect(indent(TREE, "a")).toBe(TREE);
    expect(outdent(TREE, "a")).toBe(TREE);
    expect(outdent(TREE, "e")).toBe(TREE);
    expect(indent(TREE, "zz")).toBe(TREE);
  });

  it("project_drop_follows_the_offset_within_the_neighbours", () => {
    const flat = flatten(TREE);
    // e over d lands between c and d: 0 px keeps e's depth (0), but the next row (d) is
    // at depth 1, so it is lifted to 1, under a after b.
    expect(projectDrop(flat, "e", "d", 0)).toEqual({ parentId: "a", index: 1, depth: 1 });
    // Five indents right: clamped to the row above (c, depth 2) + 1.
    expect(projectDrop(flat, "e", "d", 24 * 5)).toEqual({ parentId: "c", index: 0, depth: 3 });
    // Staying in place with one indent right: under a, after d.
    expect(projectDrop(flat, "e", "e", 24)).toEqual({ parentId: "a", index: 2, depth: 1 });
    // Far left: the root.
    expect(projectDrop(flat, "d", "d", -24 * 5)).toEqual({ parentId: null, index: 1, depth: 0 });
    // Unknown ids.
    expect(projectDrop(flat, "zz", "d", 0)).toBeNull();
  });

  it("project_drop_clamps_to_max_depth", () => {
    const flat = flatten(TREE);
    // e over d, far right: the neighbours allow 3, maxDepth 1 allows 1.
    expect(projectDrop(flat, "e", "d", 24 * 5, 24, 1)).toEqual({
      parentId: "a",
      index: 1,
      depth: 1,
    });
    // A node with descendants counts their depth: b carries c, so with maxDepth 2 b can
    // go no deeper than 1.
    expect(projectDrop(flat, "b", "d", 24 * 5, 24, 2)?.depth).toBe(1);
    // maxDepth 0 keeps everything at the root.
    expect(projectDrop(flat, "e", "d", 24 * 5, 24, 0)?.depth).toBe(0);
    // A custom indent width: 40 px is one level.
    expect(projectDrop(flat, "e", "e", 40, 40)?.depth).toBe(1);
  });

  it("project_drop_skips_parents_that_cannot_nest", () => {
    const flat = flatten(TREE);
    // e over e, one indent right would go under a; a refuses children, so it stays a root.
    expect(projectDrop(flat, "e", "e", 24, 24, Infinity, (id) => id !== "a")).toEqual({
      parentId: null,
      index: 1,
      depth: 0,
    });
    // e over d must land at depth 1 (d, the next row, is at 1), whose parent is a; when a
    // refuses children there is no allowed level, so there is no move.
    expect(projectDrop(flat, "e", "d", 0, 24, Infinity, (id) => id !== "a")).toBeNull();
    // Where a lower level is allowed it is used: b (with c) over d, after d, may go to
    // the root instead of under a.
    expect(projectDrop(flat, "b", "d", 0, 24, 2, (id) => id !== "a")).toEqual({
      parentId: null,
      index: 1,
      depth: 0,
    });
  });
});
