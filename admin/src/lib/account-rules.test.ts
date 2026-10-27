import { expect, test } from "vitest";
import { displayNameProblem, isEmailShape, passwordProblem } from "./account-rules";

test("account_rules_match_core", () => {
  expect(displayNameProblem("")).toBe("required");
  expect(displayNameProblem("é".repeat(100))).toBeNull();
  expect(displayNameProblem("é".repeat(101))).toBe("long");
  expect(isEmailShape("a@b")).toBe(true);
  expect(isEmailShape("a b@c")).toBe(false);
  expect(isEmailShape("a@b@c")).toBe(false);
  expect(passwordProblem("eleven-char")).toBe("short");
  expect(passwordProblem("twelve-chars")).toBeNull();
  expect(passwordProblem("€".repeat(400))).toBe("long");
});
