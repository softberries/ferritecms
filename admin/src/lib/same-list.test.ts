import { expect, test } from "vitest";
import { sameList } from "./same-list";

test("same_list_compares_items_in_order", () => {
  expect(sameList([], [])).toBe(true);
  expect(sameList(["a", "b"], ["a", "b"])).toBe(true);
  expect(sameList(["a", "b"], ["b", "a"])).toBe(false);
  expect(sameList(["a"], ["a", "b"])).toBe(false);
  expect(sameList([1, 2], [1, 2])).toBe(true);
});
