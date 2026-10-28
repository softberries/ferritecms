// A relation searches only the types its field lists (P52), in the sidebar's order.

import { expect, test } from "vitest";
import { searchTypes } from "./fields-model";

const NAV = [
  { name: "post", label: "Posts" },
  { name: "page", label: "Pages" },
  { name: "book", label: "Books" },
];

test("relation_searches_only_the_listed_types", () => {
  // In the sidebar's order, whatever the field's order; one the viewer cannot see is not
  // searched.
  expect(searchTypes(NAV, ["book", "post", "hidden"]).map((ty) => ty.name)).toEqual([
    "post",
    "book",
  ]);
  // None listed: any type the sidebar shows.
  expect(searchTypes(NAV, []).map((ty) => ty.name)).toEqual(["post", "page", "book"]);
  expect(searchTypes(NAV, undefined)).toEqual(NAV);
});
