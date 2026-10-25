// Fails when `src/api/schema.d.ts` is not what `pnpm api:gen` generates from the
// checked-in spec (`crates/ferrite-http/openapi.json`), printing the difference.
// Drift is a regenerate-and-diff (M2b P6), not openapi-typescript's `--check`.
// Regenerate with `pnpm api:gen` after `FERRITE_UPDATE_OPENAPI=1 cargo test -p
// ferrite-http --test api openapi`.

import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { createTwoFilesPatch } from "diff";

const root = fileURLToPath(new URL("..", import.meta.url));
const spec = join(root, "../crates/ferrite-http/openapi.json");
const committed = join(root, "src/api/schema.d.ts");
const cli = join(root, "node_modules/openapi-typescript/bin/cli.js");

const dir = mkdtempSync(join(tmpdir(), "api-check-"));
try {
  const fresh = join(dir, "schema.d.ts");
  // The same arguments as the `api:gen` script.
  execFileSync(process.execPath, [cli, spec, "-o", fresh, "--root-types"], {
    stdio: ["ignore", "ignore", "inherit"],
  });
  const want = readFileSync(fresh, "utf8");
  let have = "";
  try {
    have = readFileSync(committed, "utf8");
  } catch {
    // A missing file is reported as a difference below.
  }
  if (have !== want) {
    console.error("api:check: src/api/schema.d.ts differs from the spec; run `pnpm api:gen`.");
    console.error(createTwoFilesPatch("src/api/schema.d.ts", "generated", have, want));
    process.exitCode = 1;
  } else {
    console.log("api:check: src/api/schema.d.ts is current");
  }
} finally {
  rmSync(dir, { recursive: true, force: true });
}
