// The E2E matrix (M2b P55): four projects, each against a site of its own, so engines and
// backends never share state. `playwright.config.ts` builds the Playwright projects from
// this list; global setup starts the sites of the projects a run selects.
//
// Each project is two Playwright projects: `<name>` (every spec but the site-wide ones,
// in parallel) and its teardown `<name>-site-wide` (the site-wide specs, one worker, once
// `<name>` has finished, pass or fail). Selecting `<name>` runs both. The site-wide specs
// change state that other specs read (settings, the active theme, a dead job, an import),
// so nothing else runs beside them.

/** A site's database. */
export type Backend = "sqlite" | "pg";
export type Engine = "chromium" | "firefox" | "webkit";

/** The specs that change site-wide state; they run last, one at a time. */
export const SITE_WIDE_SPECS =
  /(?:^|[/\\])(?:import|settings|themes|system|plugins-trust|multilingual)\.spec\.ts$/;
export const SITE_WIDE_SUFFIX = "-site-wide";

export interface SiteProject {
  /** The project name, and the key of its site, storage states and log. */
  name: string;
  backend: Backend;
  engine: Engine;
}

export const SITE_PROJECTS: readonly SiteProject[] = [
  { name: "chromium-sqlite", backend: "sqlite", engine: "chromium" },
  { name: "chromium-pg", backend: "pg", engine: "chromium" },
  { name: "firefox-sqlite", backend: "sqlite", engine: "firefox" },
  { name: "webkit-sqlite", backend: "sqlite", engine: "webkit" },
];

/** The project that takes the review screenshots and writes the contact sheet. */
export const SNAP_PROJECT = "chromium-sqlite";

/**
 * The projects this environment defines: `chromium-pg` with `FERRITE_TEST_PG_URL`, and
 * always in CI (a CI job that selects it without the URL fails in global setup instead of
 * skipping; 10 §2).
 */
export function definedProjects(env: NodeJS.ProcessEnv = process.env): SiteProject[] {
  const pg = !!env.FERRITE_TEST_PG_URL || !!env.CI;
  return SITE_PROJECTS.filter((p) => p.backend !== "pg" || pg);
}

/** `--project` values in the runner's command line (`--project a b`, `--project=a`). */
export function projectArgs(argv: readonly string[]): string[] {
  const out: string[] = [];
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i] ?? "";
    if (a.startsWith("--project=")) {
      out.push(a.slice("--project=".length));
    } else if (a === "--project") {
      // Variadic, as Playwright parses it.
      while (i + 1 < argv.length && !(argv[i + 1] ?? "").startsWith("-")) out.push(argv[++i]!);
    }
  }
  return out;
}

const glob = (pattern: string) =>
  new RegExp(
    `^${pattern
      .split("*")
      .map((s) => s.replace(/[.+?^${}()|[\]\\]/g, "\\$&"))
      .join(".*")}$`,
    "i",
  );

/**
 * The site projects a run selects: those whose name, or `-site-wide` twin, matches a
 * `--project` value (with `*` wildcards, as Playwright matches them); every defined
 * project when there is none.
 */
export function selectedProjects(argv: readonly string[] = process.argv): SiteProject[] {
  const defined = definedProjects();
  const args = projectArgs(argv);
  if (args.length === 0) return defined;
  const patterns = args.map(glob);
  return defined.filter((p) =>
    patterns.some((re) => re.test(p.name) || re.test(`${p.name}${SITE_WIDE_SUFFIX}`)),
  );
}

// The public starter themes' accessibility checks (M4-3b P64, ruling R-b16): one more
// Playwright project, Chromium on SQLite, that runs `public-themes.spec.ts` and nothing
// else (the admin projects ignore that spec). It has no `-site-wide` twin and none of the
// admin projects' sites: global setup starts one site per theme, each with the content
// its theme is checked on, so no theme's content or front page leaks into another's.

/** What a site holds besides the harness accounts. */
export type SiteContent =
  /** `seed demo`. */
  | "demo"
  /** `seed demo --profile business`: a static front page, its children, a contact page. */
  | "business"
  /** The docs site: `import markdown <repo>/site/content --front-page` (`site/build.sh`). */
  | "docs";

export const PUBLIC_THEMES_PROJECT = "public-themes-chromium";
export const PUBLIC_THEMES_SPEC = /(?:^|[/\\])public-themes\.spec\.ts$/;

/** The starter themes and the content each is checked on (P64). */
export const PUBLIC_THEMES = [
  { theme: "blog", content: "demo" },
  { theme: "business", content: "business" },
  { theme: "docs", content: "docs" },
] as const satisfies readonly { theme: string; content: SiteContent }[];

export type PublicTheme = (typeof PUBLIC_THEMES)[number]["theme"];

/** The key of a theme's site, storage states and log. */
export function themeSiteKey(theme: PublicTheme): string {
  return `${PUBLIC_THEMES_PROJECT}-${theme}`;
}

/** Whether a run selects the public-themes project (it does when no project is named). */
export function publicThemesSelected(argv: readonly string[] = process.argv): boolean {
  const args = projectArgs(argv);
  return args.length === 0 || args.map(glob).some((re) => re.test(PUBLIC_THEMES_PROJECT));
}
