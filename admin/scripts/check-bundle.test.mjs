import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { randomBytes } from "node:crypto";
import { afterEach, expect, test } from "vitest";
import { checkBundle } from "./check-bundle.mjs";

/** @type {string[]} */
const dirs = [];

/** @param {Record<string, string | Buffer>} files */
function dist(files) {
  const dir = mkdtempSync(join(tmpdir(), "check-bundle-"));
  dirs.push(dir);
  for (const [name, body] of Object.entries(files)) {
    mkdirSync(dirname(join(dir, name)), { recursive: true });
    writeFileSync(join(dir, name), body);
  }
  return dir;
}

afterEach(() => {
  for (const dir of dirs.splice(0)) rmSync(dir, { recursive: true, force: true });
});

// Incompressible bytes, so the gzip size is close to the byte count.
const noise = (/** @type {number} */ n) => randomBytes(n);

const FILES = {
  "index.html": [
    '<script type="module" crossorigin src="/admin/assets/index-AAAAAAAA.js"></script>',
    '<link rel="modulepreload" crossorigin href="/admin/assets/react-BBBBBBBB.js">',
    '<link rel="stylesheet" crossorigin href="/admin/assets/index-CCCCCCCC.css">',
  ].join("\n"),
  "assets/index-AAAAAAAA.js": noise(2000),
  "assets/react-BBBBBBBB.js": noise(3000),
  "assets/index-CCCCCCCC.css": noise(1000),
  "assets/editor-DDDDDDDD.js": noise(4000),
  "assets/chart-EEEEEEEE.js": noise(5000),
  "assets/media.index-FFFFFFFF.js": noise(1500),
  "assets/users.index-GGGGGGGG.js": noise(2500),
};

const ROOMY = {
  entry: 10_000,
  entry_preloads: 10_000,
  css: 10_000,
  editor: 10_000,
  chart: 10_000,
  largest_lazy: 10_000,
};

test("check_bundle_flags_over_limit", () => {
  const d = dist(FILES);

  const ok = checkBundle(d, ROOMY);
  expect(ok.failures).toEqual([]);
  const size = Object.fromEntries(ok.rows.map((r) => [r.group, r.size]));
  // the largest lazy chunk is neither entry, preload, editor nor chart: users.index
  expect(size.largest_lazy).toBeGreaterThan(2450);
  expect(size.largest_lazy).toBeLessThan(2600);
  expect(size.entry_preloads).toBeGreaterThan(size.entry ?? 0);

  const over = checkBundle(d, { ...ROOMY, chart: 100, css: 100 });
  expect(over.failures.map((f) => f.group)).toEqual(["css", "chart"]);
  expect(over.rows.find((r) => r.group === "chart")?.headroom).toBeLessThan(0);
});

test("missing_editor_chunk_is_a_problem", () => {
  const rest = Object.fromEntries(
    Object.entries(FILES).filter(([name]) => !name.includes("editor-")),
  );
  expect(() => checkBundle(dist(rest), ROOMY)).toThrow(/editor/);
});
