import { describe, expect, test } from "vitest";
import { parseSearch, stringifySearch } from "./search-params";

describe("search_params_test", () => {
  test("free_text_stays_a_string", () => {
    for (const q of ["2024", "531038e6", "1e3", "-1", "true", "null", '"quoted"', "[1]"]) {
      expect(parseSearch(`?q=${encodeURIComponent(q)}`)).toEqual({ q });
      expect(stringifySearch({ q })).toBe(`?${new URLSearchParams({ q }).toString()}`);
      expect(parseSearch(stringifySearch({ q }))).toEqual({ q });
    }
  });

  test("other_values_decode_as_before", () => {
    expect(parseSearch("?mine=true&status=draft&n=2&q=x")).toEqual({
      mine: true,
      status: "draft",
      n: 2,
      q: "x",
    });
    expect(parseSearch(stringifySearch({ new: true, section: "colour", q: "a b" }))).toEqual({
      new: true,
      section: "colour",
      q: "a b",
    });
    expect(stringifySearch({})).toBe("");
    expect(stringifySearch({ q: undefined, mine: true })).toBe("?mine=true");
  });
});
