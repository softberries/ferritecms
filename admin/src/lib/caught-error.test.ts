import { afterEach, expect, test, vi } from "vitest";
import { ApiError } from "@/api/problem";
import { reportCaughtError } from "./caught-error";

afterEach(() => {
  vi.restoreAllMocks();
});

test("api_errors_are_not_reported_anything_else_is", () => {
  const log = vi.spyOn(console, "error").mockImplementation(() => {});
  const info = { componentStack: "\n    at Route" };
  reportCaughtError(
    new ApiError({ type: "about:blank", title: "Forbidden", status: 403, detail: "no" }),
    info,
  );
  expect(log).not.toHaveBeenCalled();
  const bug = new TypeError("x is undefined");
  reportCaughtError(bug, info);
  expect(log).toHaveBeenCalledWith(bug, info.componentStack);
});
