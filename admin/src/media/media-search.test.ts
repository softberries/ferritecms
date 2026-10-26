import { afterEach, describe, expect, test } from "vitest";
import { MEDIA_VIEW_KEY } from "@/design/preferences";
import {
  effectiveView,
  hasFilters,
  mediaQuery,
  parseMediaSearch,
  rememberView,
  searchDisables,
} from "./media-search";

const ID = "0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b";
const ME = "0190a1b2-0000-7000-8000-000000000001";

afterEach(() => {
  window.localStorage.clear();
});

describe("media_search_test", () => {
  test("defaults are omitted", () => {
    expect(parseMediaSearch({})).toEqual({});
    expect(
      parseMediaSearch({ view: "grid", q: "  ", mine: false, trash: false, kind: undefined }),
    ).toEqual({});
  });

  test("valid members are kept", () => {
    expect(
      parseMediaSearch({ view: "list", q: " cat ", kind: "image", mine: true, id: ID }),
    ).toEqual({ view: "list", q: "cat", kind: "image", mine: true, id: ID });
    expect(parseMediaSearch({ trash: "true", kind: "document" })).toEqual({
      trash: true,
      kind: "document",
    });
    // The router JSON-decodes values: a numeric search still works.
    expect(parseMediaSearch({ q: 2026, mine: "true" })).toEqual({ q: "2026", mine: true });
    expect(parseMediaSearch({ id: ID.toUpperCase() })).toEqual({ id: ID });
  });

  test("invalid params are dropped", () => {
    expect(
      parseMediaSearch({
        view: "tiles",
        kind: "spreadsheet",
        mine: "yes",
        trash: "maybe",
        id: "not-a-uuid",
        q: { nested: true },
        extra: 1,
      }),
    ).toEqual({});
    expect(parseMediaSearch({ kind: "nope", q: "kept" })).toEqual({ q: "kept" });
  });

  test("q disables mine", () => {
    const search = parseMediaSearch({ q: "cat", mine: true, kind: "image" });
    expect(search).toEqual({ q: "cat", mine: true, kind: "image" });
    expect(searchDisables(search)).toBe(true);
    // `q` with `author` is 422 on the server: the author is not sent; `kind` combines.
    expect(mediaQuery(search, ME)).toEqual({ q: "cat", kind: "image" });
    expect(searchDisables({ mine: true })).toBe(false);
  });

  test("mine maps to the author's id", () => {
    expect(mediaQuery({}, ME)).toEqual({});
    expect(mediaQuery({ mine: true, kind: "video" }, ME)).toEqual({ author: ME, kind: "video" });
  });

  test("trash has no search", () => {
    // Trashed items leave the search index (07 §1): `q` is never sent with `trash`.
    const search = parseMediaSearch({ trash: true, q: "cat", mine: true, kind: "audio" });
    expect(mediaQuery(search, ME)).toEqual({ trash: true, author: ME, kind: "audio" });
  });

  test("view and id are not filters", () => {
    expect(mediaQuery({ view: "list", id: ID }, ME)).toEqual({});
    expect(hasFilters({ view: "list", id: ID, trash: true })).toBe(false);
    expect(hasFilters({ kind: "image" })).toBe(true);
    expect(hasFilters({ mine: true })).toBe(true);
    expect(hasFilters({ q: "x" })).toBe(true);
    // In the trash a leftover `q` is not a filter (it is not sent).
    expect(hasFilters({ trash: true, q: "x" })).toBe(false);
  });

  test("view is remembered per viewer", () => {
    expect(effectiveView({})).toBe("grid");
    rememberView("list");
    expect(window.localStorage.getItem(MEDIA_VIEW_KEY)).toBe("list");
    expect(effectiveView({})).toBe("list");
    // The URL wins over the remembered view.
    expect(effectiveView({ view: "grid" })).toBe("grid");
    rememberView("grid");
    expect(effectiveView({})).toBe("grid");
    window.localStorage.setItem(MEDIA_VIEW_KEY, "bogus");
    expect(effectiveView({})).toBe("grid");
  });
});
