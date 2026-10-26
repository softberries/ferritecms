import { describe, expect, test } from "vitest";
import { parseSearch, stringifySearch } from "@/lib/search-params";
import { commentQuery, parseCommentSearch, withStatus } from "./comment-search";

const ITEM = "0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b";

describe("comment_search_test", () => {
  test("pending is the absent default", () => {
    expect(parseCommentSearch({})).toEqual({});
    expect(parseCommentSearch({ status: "pending" })).toEqual({});
    expect(commentQuery({})).toEqual({ status: "pending", limit: 50 });
  });

  test("valid members are kept", () => {
    expect(parseCommentSearch({ status: "spam", item: ITEM })).toEqual({
      status: "spam",
      item: ITEM,
    });
    expect(parseCommentSearch({ status: "approved", item: ITEM.toUpperCase() })).toEqual({
      status: "approved",
      item: ITEM,
    });
    expect(commentQuery({ status: "trash", item: ITEM })).toEqual({
      status: "trash",
      item: ITEM,
      limit: 50,
    });
  });

  test("invalid members are dropped, never an error", () => {
    expect(parseCommentSearch({ status: "all", item: "nope" })).toEqual({});
    expect(parseCommentSearch({ status: 3, item: 7 })).toEqual({});
    expect(parseCommentSearch({ status: "approved", junk: "x" })).toEqual({ status: "approved" });
  });

  test("a tab keeps the item filter", () => {
    expect(withStatus({ item: ITEM }, "spam")).toEqual({ status: "spam", item: ITEM });
    expect(withStatus({ status: "spam", item: ITEM }, "pending")).toEqual({ item: ITEM });
  });
});

describe("free_text_search", () => {
  // The queue has no free text; its item id survives the router even when it starts like
  // a number in exponent notation.
  test("an_item_id_that_looks_numeric_stays_a_string", () => {
    const item = "123e4567-e89b-42d3-a456-426614174000";
    const parsed = parseCommentSearch(parseSearch(`?item=${item}`));
    expect(parsed).toEqual({ item });
    expect(parseCommentSearch(parseSearch(stringifySearch(parsed)))).toEqual({ item });
  });
});
