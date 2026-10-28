// Unit tests (Vitest, the `scripts` project); not `*.test.ts`, which Playwright would
// collect as a spec.

import { expect, test } from "vitest";
import { childEnv } from "./child-env";

test("harness_variables_never_reach_ferrite", () => {
  // `ferrite` refuses an unknown FERRITE_* key, so FERRITE_BIN broke every run that set
  // it (seen running the Linux binary in the Playwright image).
  const env = childEnv(
    {
      PATH: "/usr/bin",
      FERRITE_BIN: "/linux-bin/ferrite",
      FERRITE_E2E_SITES: "{}",
      FERRITE_LOG: "debug",
      FERRITE_TEST_PG_URL: "postgres://x",
    },
    { FERRITE_SERVER__LISTEN: "127.0.0.1:1" },
  );
  expect(env).toEqual({
    PATH: "/usr/bin",
    FERRITE_LOG: "debug",
    FERRITE_TEST_PG_URL: "postgres://x",
    FERRITE_SERVER__LISTEN: "127.0.0.1:1",
  });
});
