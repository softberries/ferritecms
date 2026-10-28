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
import { cancelledInConsole, cancelledInPageError, type CancelledLoad } from "./webkit-cancelled";

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
 * handles (no results, the list read again) but the browser logs. Accepted in every test,
 * except a plugin's metrics in `plugins.spec.ts`, whose tests read the metrics of their own
 * plugins (`goneElsewhere`). A stored media file (`/_media/<sha256>/<name>`) is accepted only
 * in `media.spec.ts` and `editor-media.spec.ts`: the library and the media picker list every
 * item on the site, so a thumbnail of an image another test purged answers 404; elsewhere a
 * page shows only its own images, and a 404 there is a real failure.
 */
const GONE_ELSEWHERE = [
  /\/api\/v1\/content\/(?!(?:post|page|media|refs)(?:[/?]|$))[a-z][a-z0-9_-]*(?:\/counts)?(?:\?|$)/,
  /\/api\/v1\/taxonomies\/(?!(?:category|post_tag)\/)[a-z][a-z0-9_-]*\/terms(?:\?|$)/,
];
const PLUGIN_METRICS = /\/api\/v1\/plugins\/[a-z0-9][a-z0-9_-]*\/metrics(?:\?|$)/;
const MEDIA_FILE = /\/_media\/[0-9a-f]{64}\/[^/?#]+$/;

/** The `GONE_ELSEWHERE` patterns for the spec file `file`. */
const goneElsewhere = (file: string): RegExp[] => [
  ...GONE_ELSEWHERE,
  ...(/(?:^|[/\\])plugins\.spec\.ts$/.test(file) ? [] : [PLUGIN_METRICS]),
  ...(/(?:^|[/\\])(?:editor-)?media\.spec\.ts$/.test(file) ? [MEDIA_FILE] : []),
];

/**
 * What an engine reports when a navigation cancels a load still in flight, which Chromium
 * does silently (10 §3):
 * - Firefox logs a console error for a web font whose download was aborted
 *   (`status=2152398850` is NS_BINDING_ABORTED);
 * - WebKit reports a same-origin `fetch` the navigation cancelled as "Fetch API cannot
 *   load <url> due to access control checks." (Playwright passes it on as a page error),
 *   and a lazy module whose request was cancelled as "TypeError: Importing a module
 *   script failed." (both recognised by `webkit-cancelled.ts`). Each is accepted only when
 *   that page shows the cancellation within `CANCEL_WINDOW_MS`: for a fetch (of this
 *   site), its URL failed as cancelled or the page's main frame navigated (WebKit can
 *   cancel a fetch before Playwright sees its request: no `request` or `requestfailed`
 *   event, only the error just before the navigation commits); for an import, any
 *   script request failed as cancelled.
 */
const CANCELLED_LOAD: Partial<Record<string, (text: string) => boolean>> = {
  firefox: (text) =>
    /^\[JavaScript Error: "downloadable font: download failed \(.*\): status=2152398850 /.test(
      text,
    ),
};

const CANCEL_WINDOW_MS = 2_000;

/**
 * WebKit: a `reload()` while the page is still fetching the modules it preloads can fail
 * with "Frame load interrupted" (10 §3). WebKit goes on loading the document then (a trace
 * showed three document requests for one reload), and a reload sent at once is
 * interrupted by that load in turn. The page's `reload` waits `RELOAD_SETTLE_MS` and tries
 * again, at most twice.
 */
const RELOAD_SETTLE_MS = 250;
function retryInterruptedReload(page: Page): void {
  const reload = page.reload.bind(page);
  page.reload = async (options) => {
    for (let retries = 2; ; retries--) {
      try {
        return await reload(options);
      } catch (e) {
        if (retries === 0 || !String(e).includes("Frame load interrupted")) throw e;
        await new Promise((resolve) => setTimeout(resolve, RELOAD_SETTLE_MS));
      }
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
    async ({ context, browserName, site }, use, testInfo) => {
      const cancelled = CANCELLED_LOAD[browserName];
      const isCancelledLoad = (text: string) => cancelled?.(text) ?? false;
      const gone = goneElsewhere(testInfo.file);
      const problems: string[] = [];
      // WebKit: errors that a cancelled request explains (`url` null: any script), the
      // requests each page saw cancelled, and when each page's main frame navigated.
      const pending: { page: Page; url: string | null; at: number; report: string }[] = [];
      const cancelledRequests = new Map<Page, { url: string; script: boolean; at: number }[]>();
      const navigations = new Map<Page, number[]>();
      /** WebKit's report of a cancelled load, kept until the end of the test; else false. */
      const deferCancelled = (page: Page, load: CancelledLoad | null, report: string): boolean => {
        if (browserName !== "webkit" || !load) return false;
        if (load.kind === "fetch" && !load.url.startsWith(`${site.url}/`)) return false;
        pending.push({
          page,
          url: load.kind === "fetch" ? load.url : null,
          at: Date.now(),
          report,
        });
        return true;
      };
      const allowed: RegExp[] = [];
      const allowedResources: RegExp[] = [];
      const watched = new WeakSet<BrowserContext>();
      const watchPage = (page: Page) => {
        if (browserName === "webkit") retryInterruptedReload(page);
        page.on("pageerror", (e) => {
          const report = `pageerror: ${e.message}`;
          if (deferCancelled(page, cancelledInPageError(e), report)) return;
          problems.push(report);
        });
        if (browserName === "webkit") {
          page.on("requestfailed", (r) => {
            if (r.failure()?.errorText !== "cancelled") return;
            const seen = { url: r.url(), script: r.resourceType() === "script", at: Date.now() };
            cancelledRequests.set(page, [...(cancelledRequests.get(page) ?? []), seen]);
          });
          page.on("framenavigated", (frame) => {
            if (frame !== page.mainFrame()) return;
            navigations.set(page, [...(navigations.get(page) ?? []), Date.now()]);
          });
        }
        page.on("console", (msg) => {
          if (msg.type() !== "error") return;
          const text = msg.text();
          if (allowed.some((re) => re.test(text))) return;
          if (isCancelledLoad(text)) return;
          const at = msg.location();
          if (deferCancelled(page, cancelledInConsole(text), `console.error: ${text}`)) return;
          if (
            text.startsWith("Failed to load resource") &&
            allowedResources.some((re) => re.test(at.url))
          ) {
            return;
          }
          if (
            text.startsWith("Failed to load resource") &&
            text.includes("status of 404") &&
            gone.some((re) => re.test(at.url))
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
      const near = (t: number, at: number) => Math.abs(t - at) <= CANCEL_WINDOW_MS;
      for (const { page, url, at, report } of pending) {
        const explained =
          (cancelledRequests.get(page) ?? []).some(
            (r) => near(r.at, at) && (url === null ? r.script : r.url === url),
          ) ||
          (url !== null && (navigations.get(page) ?? []).some((t) => near(t, at)));
        if (!explained) problems.push(report);
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
