import { describe, expect, test } from "vitest";
import { MAX_REDIRECT_QUERY } from "@/api/redirects";
import { parseSearch, stringifySearch } from "@/lib/search-params";
import { parseRedirectsSearch } from "./redirects-search";

describe("redirects_search_test", () => {
  test("members", () => {
    expect(parseRedirectsSearch({})).toEqual({});
    expect(parseRedirectsSearch({ q: "  old  ", new: "true" })).toEqual({ q: "old", new: true });
    expect(parseRedirectsSearch({ q: "x".repeat(MAX_REDIRECT_QUERY + 5) }).q).toHaveLength(
      MAX_REDIRECT_QUERY,
    );
    expect(parseRedirectsSearch({ q: 5, new: false })).toEqual({});
  });

  test("numeric_looking_searches_stay_strings", () => {
    for (const q of ["2024", "531038e6", "1e3"]) {
      const parsed = parseRedirectsSearch(parseSearch(`?q=${q}`));
      expect(parsed).toEqual({ q });
      expect(parseRedirectsSearch(parseSearch(stringifySearch(parsed)))).toEqual({ q });
    }
  });
});
