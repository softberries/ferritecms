// `pnpm e2e` and `pnpm e2e:all`: runs `playwright test` with the projects picked for you
// (M2b P55, `e2e/support/projects.ts`).
//
// - `pnpm e2e`: `chromium-sqlite`, and `chromium-pg` with FERRITE_TEST_PG_URL.
// - `pnpm e2e:all`: all four projects.
// - `--project <name>` picks projects instead (each brings its `-site-wide` teardown).
// - With a filter (a file, `-g`, `--last-failed`, …), each picked project is named with its
//   `-site-wide` twin and `--no-deps` is passed: Playwright otherwise runs the whole of a
//   project or teardown the filter leaves empty. Filtered runs therefore skip the ordering
//   that keeps site-wide specs apart from the rest.

import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const SITE_WIDE_SUFFIX = "-site-wide";

/** Options of `playwright test` that take a value (`playwright test --help`, 1.64). */
const VALUE_OPTIONS = new Set([
  "--add-reporter",
  "--browser",
  "-c",
  "--config",
  "-g",
  "--grep",
  "-G",
  "--grep-invert",
  "--global-timeout",
  "-j",
  "--workers",
  "--last-failed-file",
  "--max-failures",
  "--output",
  "--repeat-each",
  "--reporter",
  "--retries",
  "--run-agents",
  "--shard",
  "--test-list",
  "--test-list-invert",
  "--timeout",
  "--trace",
  "--tsconfig",
  "--ui-host",
  "--ui-port",
  "--update-source-method",
]);

/** Options that narrow the tests a run selects. */
const FILTER_OPTIONS = new Set([
  "-g",
  "--grep",
  "-G",
  "--grep-invert",
  "--last-failed",
  "--only-changed",
  "--test-list",
  "--test-list-invert",
]);

/**
 * The `playwright test` arguments for `args` (the user's), `all` (`e2e:all`) and whether
 * the Postgres project is defined (`pg`).
 *
 * @param {readonly string[]} args
 * @param {{ all: boolean; pg: boolean }} opts
 * @returns {string[]}
 */
export function playwrightArgs(args, { all, pg }) {
  /** @type {string[]} */
  const projects = [];
  /** @type {string[]} */
  const rest = [];
  let filtered = false;
  for (let i = 0; i < args.length; i++) {
    const a = args[i] ?? "";
    if (a.startsWith("--project=")) {
      projects.push(a.slice("--project=".length));
    } else if (a === "--project") {
      // Variadic, as Playwright parses it, but a path after it is a file filter.
      while (i + 1 < args.length && !/^-|[/\\]|\.ts(?::\d+)*$/.test(args[i + 1] ?? "")) {
        projects.push(args[++i] ?? "");
      }
    } else {
      const name = a.split("=")[0] ?? a;
      if (FILTER_OPTIONS.has(name)) filtered = true;
      rest.push(a);
      if (VALUE_OPTIONS.has(a) && i + 1 < args.length) {
        rest.push(args[++i] ?? "");
      } else if (!a.startsWith("-")) {
        // A positional argument: a file filter.
        filtered = true;
      }
    }
  }
  if (projects.length === 0 && !all) {
    projects.push("chromium-sqlite");
    if (pg) projects.push("chromium-pg");
  }
  const out = [...projects];
  if (filtered) {
    // `e2e:all` with a filter: name every project, so each comes with its twin.
    if (out.length === 0) {
      out.push("chromium-sqlite", "firefox-sqlite", "webkit-sqlite");
      if (pg) out.push("chromium-pg");
    }
    for (const p of [...out]) {
      if (!p.endsWith(SITE_WIDE_SUFFIX) && !p.endsWith("*")) out.push(`${p}${SITE_WIDE_SUFFIX}`);
    }
  }
  return ["test", ...out.map((p) => `--project=${p}`), ...(filtered ? ["--no-deps"] : []), ...rest];
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const argv = process.argv.slice(2);
  const all = argv[0] === "--all";
  const args = playwrightArgs(all ? argv.slice(1) : argv, {
    all,
    pg: !!process.env.FERRITE_TEST_PG_URL || !!process.env.CI,
  });
  const bin = fileURLToPath(new URL("../node_modules/.bin/playwright", import.meta.url));
  console.log(`playwright ${args.join(" ")}`);
  const r = spawnSync(bin, args, { stdio: "inherit" });
  if (r.error) throw r.error;
  process.exit(r.status ?? 1);
}
