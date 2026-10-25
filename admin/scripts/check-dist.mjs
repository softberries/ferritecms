// Checks the admin build against the CSP it is served under (07 §3, M2b Task 1):
// `script-src 'self'` (no inline script), `font-src 'self'` (no data: fonts), and
// the build settings `assetsInlineLimit: 0` (no data: images) and `sourcemap:
// false` (no .map files); no `eval(` or `new Function` in the JS (no
// `'unsafe-eval'`). Files under `assets/` must carry a content hash, since
// the server caches them as immutable. Runs at the end of `pnpm build`.

import { readFileSync, readdirSync } from "node:fs";
import { join, relative, sep } from "node:path";
import { fileURLToPath } from "node:url";

/** @param {string} dir @returns {string[]} every file below `dir`, relative, with `/` */
function walk(dir) {
  /** @type {string[]} */
  const out = [];
  for (const entry of readdirSync(dir, { withFileTypes: true, recursive: true })) {
    if (entry.isFile()) {
      out.push(relative(dir, join(entry.parentPath, entry.name)).split(sep).join("/"));
    }
  }
  return out.sort();
}

// `name-<hash>.ext`: Vite's default `[name]-[hash].[ext]`, hash of 8+ base64url characters.
const HASHED = /-[A-Za-z0-9_-]{8,}\.[A-Za-z0-9]+$/;
const SCRIPT_TAG = /<script\b([^>]*)>/gi;
const SRC_ATTR = /(?:^|\s)src\s*=/i;
const DATA_URL = /data:(?:font|image)/i;
// A call of `eval` (not `.eval(`, e.g. a method of a library object) or the
// `Function` constructor; both need `'unsafe-eval'`, which the CSP leaves out.
const EVAL = /(?<![\w$.])eval\s*\(|\bnew\s+Function\b/;

/**
 * @param {string} dist the build output folder
 * @returns {string[]} one line per problem; empty when the build is fine
 */
export function checkDist(dist) {
  /** @type {string[]} */
  const problems = [];
  const files = walk(dist);
  if (!files.includes("index.html")) {
    problems.push("index.html: missing");
  }
  for (const file of files) {
    if (file.endsWith(".map")) {
      problems.push(`${file}: source map in the build`);
      continue;
    }
    const text = readFileSync(join(dist, file), "latin1");
    if (file.endsWith(".html")) {
      for (const [, attrs] of text.matchAll(SCRIPT_TAG)) {
        if (!SRC_ATTR.test(attrs ?? "")) {
          problems.push(`${file}: inline <script> (the CSP allows only script-src 'self')`);
        }
      }
    }
    if (/\.(?:m?js)$/.test(file) && EVAL.test(text)) {
      problems.push(`${file}: uses eval or new Function (the CSP has no 'unsafe-eval')`);
    }
    if (!file.endsWith(".svg") && DATA_URL.test(text)) {
      problems.push(`${file}: contains a data: font or image URL`);
    }
    if (file.startsWith("assets/") && !HASHED.test(file)) {
      problems.push(`${file}: asset name has no content hash`);
    }
  }
  return problems;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const dist = process.argv[2] ?? fileURLToPath(new URL("../dist", import.meta.url));
  const problems = checkDist(dist);
  if (problems.length > 0) {
    console.error(`check-dist: ${problems.length} problem(s) in ${dist}`);
    for (const p of problems) console.error(`  ${p}`);
    process.exit(1);
  }
  console.log(`check-dist: ok (${dist})`);
}
