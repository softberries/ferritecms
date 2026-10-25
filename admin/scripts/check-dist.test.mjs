import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { afterEach, expect, test } from "vitest";
import { checkDist } from "./check-dist.mjs";

/** @type {string[]} */
const dirs = [];

/** @param {Record<string, string>} files */
function dist(files) {
  const dir = mkdtempSync(join(tmpdir(), "check-dist-"));
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

const GOOD = {
  "index.html":
    '<!doctype html><script type="module" crossorigin src="/admin/assets/index-B1x_9aQz.js"></script>',
  "assets/index-B1x_9aQz.js": "console.log(1)",
  "assets/index-Cq2-7bZe.css": "a{color:red}",
  "favicon.svg": '<svg><image href="data:image/png;base64,AA"/></svg>',
};

test("clean_build_passes", () => {
  expect(checkDist(dist(GOOD))).toEqual([]);
});

test("inline_script_fails", () => {
  const problems = checkDist(
    dist({ ...GOOD, "index.html": GOOD["index.html"] + "<script>alert(1)</script>" }),
  );
  expect(problems).toEqual(["index.html: inline <script> (the CSP allows only script-src 'self')"]);
});

test("data_urls_fail_outside_svg", () => {
  const problems = checkDist(
    dist({
      ...GOOD,
      "assets/index-Cq2-7bZe.css": "@font-face{src:url(data:font/woff2;base64,AA)}",
      "assets/index-B1x_9aQz.js": 'const i="data:image/png;base64,AA"',
    }),
  );
  expect(problems).toEqual([
    "assets/index-B1x_9aQz.js: contains a data: font or image URL",
    "assets/index-Cq2-7bZe.css: contains a data: font or image URL",
  ]);
});

test("source_map_fails", () => {
  const problems = checkDist(dist({ ...GOOD, "assets/index-B1x_9aQz.js.map": "{}" }));
  expect(problems).toEqual(["assets/index-B1x_9aQz.js.map: source map in the build"]);
});

test("unhashed_asset_fails", () => {
  const problems = checkDist(dist({ ...GOOD, "assets/inter-latin.woff2": "x" }));
  expect(problems).toEqual(["assets/inter-latin.woff2: asset name has no content hash"]);
});

test("missing_index_fails", () => {
  const rest = Object.fromEntries(Object.entries(GOOD).filter(([name]) => name !== "index.html"));
  expect(checkDist(dist(rest))).toEqual(["index.html: missing"]);
});
