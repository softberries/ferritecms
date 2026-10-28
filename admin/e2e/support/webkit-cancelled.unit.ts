// Unit tests (Vitest, the `scripts` project); not `*.test.ts`, which Playwright would
// collect as a spec.

import { describe, expect, it } from "vitest";
import { cancelledInConsole, cancelledInPageError } from "./webkit-cancelled";

const URL = "http://127.0.0.1:59314/api/v1/content/post?q=WebAssembly&limit=8";
const FETCH_TEXT = `Fetch API cannot load ${URL} due to access control checks.`;

describe("cancelledInPageError", () => {
  it("a_stackless_fetch_error_split_at_the_scheme_is_recognised", () => {
    // As Playwright 1.64 passes it on when WebKit sends no frames (seen in an e2e:all run).
    const e = {
      name: "Fetch API cannot load http",
      message:
        "/127.0.0.1:59314/api/v1/content/post?q=WebAssembly&limit=8 due to access control checks.",
      stack: "",
    };
    expect(cancelledInPageError(e)).toEqual({ kind: "fetch", url: URL });
  });

  it("a_fetch_error_with_frames_is_read_from_its_stack", () => {
    const e = {
      name: "Fetch API cannot load http",
      message: URL.slice("http:/".length) + " due to access control checks.",
      stack: `${FETCH_TEXT}\n    at unknown (${URL}:1:1)`,
    };
    expect(cancelledInPageError(e)).toEqual({ kind: "fetch", url: URL });
  });

  it("a_split_import_error_is_recognised_with_or_without_frames", () => {
    const e = { name: "TypeError", message: "Importing a module script failed." };
    expect(cancelledInPageError({ ...e, stack: "" })).toEqual({ kind: "import" });
    expect(
      cancelledInPageError({
        ...e,
        stack: "TypeError: Importing a module script failed.\n    at unknown (x:1:1)",
      }),
    ).toEqual({ kind: "import" });
  });

  it("other_errors_are_not_cancelled_loads", () => {
    expect(cancelledInPageError({ name: "TypeError", message: "x is undefined", stack: "" })).toBe(
      null,
    );
    expect(
      cancelledInPageError({
        name: "Fetch API cannot load http",
        message: "/127.0.0.1/x due to a network error.",
      }),
    ).toBe(null);
  });
});

describe("cancelledInConsole", () => {
  it("reads_the_first_line", () => {
    expect(cancelledInConsole(FETCH_TEXT)).toEqual({ kind: "fetch", url: URL });
    expect(cancelledInConsole("TypeError: Importing a module script failed.")).toEqual({
      kind: "import",
    });
    expect(cancelledInConsole("Something else")).toBe(null);
  });
});
