// The E2E `test`: every spec imports `test` and `expect` from here.
//
// - `siteKey` (project option, the project's name) picks the project's own site; `baseURL`
//   and `site` follow it. `backend` (project option) is that site's database.
// - `as(role)`: a page logged in as a harness account (its storage state).
// - `freshSession(role, forwardedFor)`: a page on a new session of its own, for tests that
//   log out or end sessions (pre-flight F23), from its own address (F24).
// - `snap(page, name)`: review screenshots (`snap.ts`; taken in `chromium-sqlite` only, a
//   no-op elsewhere, P55).
// - `expectAccessible(page)`: axe in the current colour scheme, then the other.
// - `guard` (automatic): fails the test on any `securitypolicyviolation`, page error or
//   `console.error` in any page of the test (Review Focus 2). `guard.allow(re)` accepts
//   console errors a test expects (for example the browser's log line for a deliberate 401);
//   `guard.allowFailedResource(url)` accepts only the browser's failed-load line of one URL.
//   A 404 of a custom type's list or counts, a custom taxonomy's terms or a plugin's
//   metrics is always accepted (`GONE_ELSEWHERE`), and so
//   is each engine's own line for a load that a navigation cancelled (`CANCELLED_LOAD`).

import AxeBuilder from "@axe-core/playwright";
import {
  test as base,
  expect,
  type BrowserContext,
  type BrowserContextOptions,
  type Page,
} from "@playwright/test";
import { SNAP_PROJECT, type Backend } from "./projects";
import { authFile, siteInfo, type SiteInfo } from "./server";
import {
  currentScheme,
  settle,
  snap as takeSnap,
  VIEWPORTS,
  type Scheme,
  type SnapOptions,
} from "./snap";
import { account, type Role } from "./users";

export { expect };
export type { Role };

export const AXE_TAGS = ["wcag2a", "wcag2aa", "wcag21a", "wcag21aa", "wcag22aa"];
/**
 * A toast fading in or out (its own timer starts that at any moment): its colours are
 * part way through the transition then, so axe leaves it out; a toast at rest is checked.
 */
export const TOAST_IN_TRANSITION =
  '[data-sonner-toast][data-removed="true"], [data-sonner-toast][data-mounted="false"]';

/**
 * Requests for what another test may delete while this one runs: the list or counts of a
 * custom content type (`/api/v1/content/<type>` or `/counts`; never a built-in type,
 * `post`, `page` or `media`, nor the static `/content/refs`, whose 404s are real
 * failures), a custom taxonomy's terms (never `category` or `post_tag`), and a plugin's
 * metrics (the seeded site has no plugins). Tests run in parallel against one site, so an
 * open page can still list a type, taxonomy or plugin another test deleted: its fan-out
 * request (the palette, the menu editor, the dashboard) answers 404, which the page
 * handles (no results, the list read again) but the browser logs. Accepted in every test.
 */
const GONE_ELSEWHERE = [
  /\/api\/v1\/content\/(?!(?:post|page|media|refs)(?:[/?]|$))[a-z][a-z0-9_-]*(?:\/counts)?(?:\?|$)/,
  /\/api\/v1\/taxonomies\/(?!(?:category|post_tag)\/)[a-z][a-z0-9_-]*\/terms(?:\?|$)/,
  /\/api\/v1\/plugins\/[a-z0-9][a-z0-9_-]*\/metrics(?:\?|$)/,
];

/**
 * What an engine reports when a navigation cancels a load still in flight, which Chromium
 * does silently (10 §3):
 * - Firefox logs a console error for a web font whose download was aborted
 *   (`status=2152398850` is NS_BINDING_ABORTED);
 * - WebKit reports a same-origin `fetch` the navigation cancelled as "Fetch API cannot
 *   load <url> due to access control checks." (Playwright passes it on as a page error),
 *   and a lazy module whose request was cancelled as "TypeError: Importing a module
 *   script failed." (accepted only when that page saw a script request cancelled within
 *   `CANCEL_WINDOW_MS`, `IMPORT_CANCELLED`).
 */
const CANCELLED_LOAD: Partial<Record<string, (text: string, origin: string) => boolean>> = {
  firefox: (text) =>
    /^\[JavaScript Error: "downloadable font: download failed \(.*\): status=2152398850 /.test(
      text,
    ),
  webkit: (text, origin) =>
    text.startsWith(`Fetch API cannot load ${origin}/`) &&
    text.split("\n")[0]?.endsWith(" due to access control checks.") === true,
};

const IMPORT_CANCELLED = "TypeError: Importing a module script failed.";
const CANCEL_WINDOW_MS = 2_000;

/**
 * WebKit: a `reload()` while the page is still fetching the modules it preloads can fail
 * with "Frame load interrupted" (the reload's own request is cancelled; 10 §3). The
 * page's `reload` tries once more then.
 */
function retryInterruptedReload(page: Page): void {
  const reload = page.reload.bind(page);
  page.reload = async (options) => {
    try {
      return await reload(options);
    } catch (e) {
      if (!String(e).includes("Frame load interrupted")) throw e;
      return reload(options);
    }
  };
}

export interface Guard {
  /** Console errors matching `re` do not fail the test. */
  allow(re: RegExp): void;
  /**
   * The browser's own "Failed to load resource" line for a request whose URL matches
   * `url` (a 404 the page expects and handles) does not fail the test; any other
   * console error still does.
   */
  allowFailedResource(url: RegExp): void;
  /** Watches a context this test created itself (`as`/`freshSession` do it already). */
  watch(context: BrowserContext): Promise<void>;
}

export interface Options {
  backend: Backend;
  /** The project's name: the key of its site and storage states (`projects.ts`). */
  siteKey: string;
}

interface Fixtures {
  site: SiteInfo;
  guard: Guard;
  as: (role: Role) => Promise<Page>;
  freshSession: (role: Role, forwardedFor: string) => Promise<Page>;
  snap: (page: Page, name: string, options?: SnapOptions) => Promise<void>;
  expectAccessible: (page: Page) => Promise<void>;
}

export const test = base.extend<Fixtures & Options>({
  backend: ["sqlite", { option: true }],
  siteKey: [SNAP_PROJECT, { option: true }],
  site: async ({ siteKey }, use) => {
    await use(siteInfo(siteKey));
  },
  baseURL: async ({ site }, use) => {
    await use(site.url);
  },

  guard: [
    async ({ context, browserName, site }, use) => {
      const cancelled = CANCELLED_LOAD[browserName];
      const isCancelledLoad = (text: string) => cancelled?.(text, site.url) ?? false;
      const problems: string[] = [];
      // WebKit: failed module imports, and when each page saw a script request cancelled.
      const imports: { page: Page; at: number }[] = [];
      const scriptsCancelled = new Map<Page, number[]>();
      const allowed: RegExp[] = [];
      const allowedResources: RegExp[] = [];
      const watched = new WeakSet<BrowserContext>();
      const watchPage = (page: Page) => {
        if (browserName === "webkit") retryInterruptedReload(page);
        page.on("pageerror", (e) => {
          if (isCancelledLoad(e.stack ?? "")) return;
          if (browserName === "webkit" && e.message === IMPORT_CANCELLED) {
            imports.push({ page, at: Date.now() });
            return;
          }
          problems.push(`pageerror: ${e.message}`);
        });
        if (browserName === "webkit") {
          page.on("requestfailed", (r) => {
            if (r.resourceType() !== "script" || r.failure()?.errorText !== "cancelled") return;
            scriptsCancelled.set(page, [...(scriptsCancelled.get(page) ?? []), Date.now()]);
          });
        }
        page.on("console", (msg) => {
          if (msg.type() !== "error") return;
          const text = msg.text();
          if (allowed.some((re) => re.test(text))) return;
          if (isCancelledLoad(text)) return;
          const at = msg.location();
          if (
            text.startsWith("Failed to load resource") &&
            allowedResources.some((re) => re.test(at.url))
          ) {
            return;
          }
          if (
            text.startsWith("Failed to load resource") &&
            text.includes("status of 404") &&
            GONE_ELSEWHERE.some((re) => re.test(at.url))
          ) {
            return;
          }
          problems.push(`console.error: ${text}${at.url ? ` (${at.url}:${at.lineNumber})` : ""}`);
        });
      };
      const guard: Guard = {
        allow: (re) => {
          allowed.push(re);
        },
        allowFailedResource: (url) => {
          allowedResources.push(url);
        },
        watch: async (ctx) => {
          if (watched.has(ctx)) return;
          watched.add(ctx);
          await ctx.exposeBinding("__ferriteCspViolation", (_src, text: string) => {
            problems.push(`securitypolicyviolation: ${text}`);
          });
          await ctx.addInitScript(() => {
            document.addEventListener(
              "securitypolicyviolation",
              (e) => {
                const report = (window as unknown as { __ferriteCspViolation: (t: string) => void })
                  .__ferriteCspViolation;
                report(
                  `${e.effectiveDirective} blocked ${e.blockedURI || "inline"} at ${e.sourceFile}:${e.lineNumber}`,
                );
              },
              true,
            );
          });
          ctx.pages().forEach(watchPage);
          ctx.on("page", watchPage);
        },
      };
      await guard.watch(context);
      await use(guard);
      for (const { page, at } of imports) {
        const near = (scriptsCancelled.get(page) ?? []).some(
          (t) => Math.abs(t - at) <= CANCEL_WINDOW_MS,
        );
        if (!near) problems.push(`pageerror: ${IMPORT_CANCELLED}`);
      }
      expect(problems, "CSP violations, page errors and console errors").toEqual([]);
    },
    { auto: true },
  ],

  as: async ({ browser, site, siteKey, guard }, use) => {
    const contexts: BrowserContext[] = [];
    await use(async (role) => {
      if (!account(role).storageState) {
        throw new Error(`${role} has no storage state; use freshSession (F23)`);
      }
      const ctx = await browser.newContext({
        ...contextDefaults(site),
        storageState: authFile(siteKey, role),
      });
      contexts.push(ctx);
      await guard.watch(ctx);
      return ctx.newPage();
    });
    await Promise.all(contexts.map((c) => c.close()));
  },

  freshSession: async ({ browser, site, guard }, use) => {
    const contexts: BrowserContext[] = [];
    await use(async (role, forwardedFor) => {
      const a = account(role);
      const password = a.password ?? site.resetmePassword;
      const ctx = await browser.newContext({
        ...contextDefaults(site),
        extraHTTPHeaders: { "X-Forwarded-For": forwardedFor },
      });
      contexts.push(ctx);
      await guard.watch(ctx);
      const res = await ctx.request.post("/api/v1/auth/login", {
        headers: { Origin: site.url },
        data: { login: a.login, password },
      });
      expect(res.status(), `login of ${role}`).toBe(200);
      return ctx.newPage();
    });
    await Promise.all(contexts.map((c) => c.close()));
  },

  snap: async ({ siteKey }, use) => {
    await use(async (page, name, options) => {
      // One set of review screenshots per run, from Chromium on SQLite (P55); the other
      // projects only check behaviour (global setup logs that once).
      if (siteKey === SNAP_PROJECT) await takeSnap(page, name, options);
    });
  },

  expectAccessible: async ({}, use) => {
    await use(async (page) => {
      const first = await currentScheme(page);
      const order: Scheme[] = first === "dark" ? ["dark", "light"] : ["light", "dark"];
      try {
        for (const scheme of order) {
          await page.emulateMedia({ colorScheme: scheme });
          await settle(page);
          const result = await new AxeBuilder({ page })
            .withTags(AXE_TAGS)
            .exclude(TOAST_IN_TRANSITION)
            .analyze();
          const violations = result.violations.map((v) => ({
            id: v.id,
            impact: v.impact,
            help: v.help,
            targets: v.nodes.map((n) => n.target.join(" ")),
          }));
          expect(violations, `axe violations in ${scheme} mode`).toEqual([]);
        }
      } finally {
        await page.emulateMedia({ colorScheme: first });
      }
    });
  },
});

function contextDefaults(site: SiteInfo): BrowserContextOptions {
  return { baseURL: site.url, viewport: VIEWPORTS.desktop, colorScheme: "light" };
}
