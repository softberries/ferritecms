import { describe, expect, test } from "vitest";
import {
  LIST_PAGE_SIZE,
  hasFilters,
  listQuery,
  parseListSearch,
  searchDisables,
} from "./list-search";
import { parseSearch, stringifySearch } from "@/lib/search-params";

const TERM = "0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b";
const ME = "0190a1b2-0000-7000-8000-000000000001";

describe("list_search_test", () => {
  test("defaults are omitted", () => {
    expect(parseListSearch({})).toEqual({});
    expect(parseListSearch({ status: "all", order: "updated", mine: false, q: "" })).toEqual({});
    // Whitespace alone is no search.
    expect(parseListSearch({ q: "   " })).toEqual({});
    expect(Object.keys(parseListSearch({ status: "all" }))).toEqual([]);
  });

  test("valid members are kept", () => {
    expect(
      parseListSearch({ status: "draft", q: " wasm ", mine: true, term: TERM, order: "title" }),
    ).toEqual({ status: "draft", q: "wasm", mine: true, term: TERM, order: "title" });
    expect(parseListSearch({ status: "trash" })).toEqual({ status: "trash" });
    // A string boolean works too.
    expect(parseListSearch({ q: "2026", mine: "true" })).toEqual({ q: "2026", mine: true });
    expect(parseListSearch({ term: TERM.toUpperCase() })).toEqual({ term: TERM });
  });

  test("invalid params are dropped", () => {
    expect(
      parseListSearch({
        status: "deleted",
        order: "menu",
        mine: "yes",
        term: "not-a-uuid",
        q: { nested: true },
        extra: "ignored",
      }),
    ).toEqual({});
    // One bad member does not take the good ones with it.
    expect(parseListSearch({ status: "nope", q: "kept" })).toEqual({ q: "kept" });
  });

  test("q disables mine and term", () => {
    const search = parseListSearch({ q: "wasm", mine: true, term: TERM, order: "title" });
    // Still in the URL, so clearing the search brings them back ...
    expect(search).toEqual({ q: "wasm", mine: true, term: TERM, order: "title" });
    expect(searchDisables(search)).toBe(true);
    // ... but never sent with `q` (07 §1: 422 otherwise); the server ignores `order`.
    expect(listQuery(search, ME)).toEqual({ limit: LIST_PAGE_SIZE, q: "wasm" });
    expect(searchDisables({ mine: true })).toBe(false);
  });

  test("list query maps mine to the author's id", () => {
    expect(listQuery({}, ME)).toEqual({ limit: 50, order: "updated" });
    expect(
      listQuery({ status: "pending", mine: true, term: TERM, order: "published" }, ME),
    ).toEqual({ limit: 50, status: "pending", author: ME, term: TERM, order: "published" });
  });

  test("has filters", () => {
    expect(hasFilters({})).toBe(false);
    expect(hasFilters({ status: "draft", order: "title" })).toBe(false);
    expect(hasFilters({ mine: true })).toBe(true);
    expect(hasFilters({ q: "x" })).toBe(true);
    expect(hasFilters({ term: TERM })).toBe(true);
  });
});

describe("free_text_search", () => {
  test("numeric_looking_searches_stay_strings", () => {
    for (const q of ["2024", "531038e6", "1e3"]) {
      const parsed = parseListSearch(parseSearch(`?q=${q}`));
      expect(parsed).toEqual({ q });
      expect(parseListSearch(parseSearch(stringifySearch(parsed)))).toEqual({ q });
    }
  });
});
