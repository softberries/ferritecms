import { expect, test } from "vitest";
import { parseUsersSearch } from "./users-search";

test("users_search_keeps_valid_members", () => {
  expect(
    parseUsersSearch({ q: "  ada ", role: "editor", status: "disabled", id: "x", new: "true" }),
  ).toEqual({ q: "ada", role: "editor", status: "disabled", id: "x", new: true });
  expect(parseUsersSearch({ q: "   ", role: "owner", status: "gone", id: 3 })).toEqual({});
  expect(parseUsersSearch({ q: "a".repeat(150) }).q).toHaveLength(100);
});
