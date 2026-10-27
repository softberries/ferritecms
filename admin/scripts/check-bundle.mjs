// Bundle-size budget (M2b-3 Task 15, P56). Reads `dist/index.html` for the entry
// script, its modulepreloads and the main stylesheet, finds the editor (TipTap) and
// chart chunks by name prefix, gzips each at level 9 and compares the sizes with
// `bundle-budget.json`. Exits 1 naming every group over its limit. Run after
// `pnpm build`; to raise a limit, edit `bundle-budget.json` in a reviewed change.

import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { gzipSync } from "node:zlib";

/** @typedef {"entry"|"entry_preloads"|"css"|"editor"|"chart"|"largest_lazy"} Group */
/** @type {Group[]} */
const GROUPS = ["entry", "entry_preloads", "css", "editor", "chart", "largest_lazy"];

// `name-<hash>.js`: Vite's `[name]-[hash].[ext]`, a hash of exactly 8 base64url characters.
const chunk = (/** @type {string} */ prefix) => new RegExp(`^${prefix}-[A-Za-z0-9_-]{8}\\.js$`);
const EDITOR = chunk("editor");
const CHART = chunk("chart");

/** @param {string} dist @param {string} file */
const gz = (dist, file) => gzipSync(readFileSync(join(dist, "assets", file)), { level: 9 }).length;
/** @param {string} href */
const base = (href) => href.slice(href.lastIndexOf("/") + 1);

/**
 * @param {string} dist the build output folder
 * @param {Record<Group, number>} limits gzip -9 bytes per group
 * @returns {{ rows: { group: Group, size: number, limit: number, headroom: number }[],
 *   failures: { group: Group, size: number, limit: number }[] }}
 */
export function checkBundle(dist, limits) {
  const html = readFileSync(join(dist, "index.html"), "utf8");
  const entry = html.match(/<script\b[^>]*\bsrc="([^"]+\.js)"/)?.[1];
  const css = html.match(/<link\b[^>]*rel="stylesheet"[^>]*\bhref="([^"]+\.css)"/)?.[1];
  if (!entry) throw new Error("index.html: no entry script");
  if (!css) throw new Error("index.html: no stylesheet");
  const preloads = [...html.matchAll(/<link\b[^>]*rel="modulepreload"[^>]*\bhref="([^"]+)"/g)].map(
    (m) => base(m[1] ?? ""),
  );
  const entryFile = base(entry);

  const assets = readdirSync(join(dist, "assets")).filter((f) => f.endsWith(".js"));
  const editor = assets.filter((f) => EDITOR.test(f));
  const chart = assets.filter((f) => CHART.test(f));
  if (editor.length !== 1)
    throw new Error(`expected one editor-*.js chunk, found ${editor.length}`);
  if (chart.length !== 1) throw new Error(`expected one chart-*.js chunk, found ${chart.length}`);
  const skip = new Set([entryFile, ...preloads, ...editor, ...chart]);
  const lazy = assets.filter((f) => !skip.has(f)).map((f) => gz(dist, f));

  /** @type {Record<Group, number>} */
  const sizes = {
    entry: gz(dist, entryFile),
    entry_preloads: [entryFile, ...preloads].reduce((sum, f) => sum + gz(dist, f), 0),
    css: gzipSync(readFileSync(join(dist, "assets", base(css))), { level: 9 }).length,
    editor: gz(dist, editor[0] ?? ""),
    chart: gz(dist, chart[0] ?? ""),
    largest_lazy: Math.max(0, ...lazy),
  };
  const rows = GROUPS.map((group) => ({
    group,
    size: sizes[group],
    limit: limits[group],
    headroom: limits[group] - sizes[group],
  }));
  return {
    rows,
    failures: rows
      .filter((r) => r.headroom < 0)
      .map(({ group, size, limit }) => ({ group, size, limit })),
  };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const dist = process.argv[2] ?? fileURLToPath(new URL("../dist", import.meta.url));
  const budget = JSON.parse(
    readFileSync(fileURLToPath(new URL("../bundle-budget.json", import.meta.url)), "utf8"),
  );
  const { rows, failures } = checkBundle(dist, budget.limits);
  const pad = (/** @type {string|number} */ v, /** @type {number} */ n) => String(v).padStart(n);
  console.log(`bundle (gzip -9 bytes; limits measured at ${budget.measured_at})`);
  console.log(`${"group".padEnd(15)}${pad("size", 10)}${pad("limit", 10)}${pad("headroom", 10)}`);
  for (const r of rows) {
    console.log(`${r.group.padEnd(15)}${pad(r.size, 10)}${pad(r.limit, 10)}${pad(r.headroom, 10)}`);
  }
  if (failures.length > 0) {
    for (const f of failures) {
      console.error(`bundle:check: ${f.group} is ${f.size} bytes, over its limit of ${f.limit}`);
    }
    console.error("Raise a limit only in a reviewed edit of admin/bundle-budget.json.");
    process.exit(1);
  }
  console.log("bundle:check: ok");
}
