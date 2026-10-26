// The E2E `test`: every spec imports `test` and `expect` from here.
//
// - `backend` (project option) picks the site; `baseURL` and `site` follow it.
// - `as(role)`: a page logged in as a harness account (its storage state).
// - `freshSession(role, forwardedFor)`: a page on a new session of its own, for tests that
//   log out or end sessions (pre-flight F23), from its own address (F24).
// - `snap(page, name)`: review screenshots (`snap.ts`; written by the SQLite project only).
// - `expectAccessible(page)`: axe in the current colour scheme, then the other.
// - `guard` (automatic): fails the test on any `securitypolicyviolation`, page error or
//   `console.error` in any page of the test (Review Focus 2). `guard.allow(re)` accepts
//   console errors a test expects (for example the browser's log line for a deliberate 401);
//   `guard.allowFailedResource(url)` accepts only the browser's failed-load line of one URL.

import AxeBuilder from "@axe-core/playwright";
import {
  test as base,
  expect,
  type BrowserContext,
  type BrowserContextOptions,
  type Page,
} from "@playwright/test";
import { authFile, siteInfo, type Backend, type SiteInfo } from "./server";
import { currentScheme, settle, snap as takeSnap, VIEWPORTS, type Scheme } from "./snap";
import { account, type Role } from "./users";

export { expect };
export type { Role };

export const AXE_TAGS = ["wcag2a", "wcag2aa", "wcag21a", "wcag21aa", "wcag22aa"];

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
}

interface Fixtures {
  site: SiteInfo;
  guard: Guard;
  as: (role: Role) => Promise<Page>;
  freshSession: (role: Role, forwardedFor: string) => Promise<Page>;
  snap: (page: Page, name: string) => Promise<void>;
  expectAccessible: (page: Page) => Promise<void>;
}

export const test = base.extend<Fixtures & Options>({
  backend: ["sqlite", { option: true }],
  site: async ({ backend }, use) => {
    await use(siteInfo(backend));
  },
  baseURL: async ({ site }, use) => {
    await use(site.url);
  },

  guard: [
    async ({ context }, use) => {
      const problems: string[] = [];
      const allowed: RegExp[] = [];
      const allowedResources: RegExp[] = [];
      const watched = new WeakSet<BrowserContext>();
      const watchPage = (page: Page) => {
        page.on("pageerror", (e) => problems.push(`pageerror: ${e.message}`));
        page.on("console", (msg) => {
          if (msg.type() !== "error") return;
          const text = msg.text();
          if (allowed.some((re) => re.test(text))) return;
          const at = msg.location();
          if (
            text.startsWith("Failed to load resource") &&
            allowedResources.some((re) => re.test(at.url))
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
      expect(problems, "CSP violations, page errors and console errors").toEqual([]);
    },
    { auto: true },
  ],

  as: async ({ browser, site, backend, guard }, use) => {
    const contexts: BrowserContext[] = [];
    await use(async (role) => {
      if (!account(role).storageState) {
        throw new Error(`${role} has no storage state; use freshSession (F23)`);
      }
      const ctx = await browser.newContext({
        ...contextDefaults(site),
        storageState: authFile(backend, role),
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

  snap: async ({ backend }, use) => {
    await use(async (page, name) => {
      // One set of review screenshots per run; the Postgres smoke project only checks
      // behaviour.
      if (backend === "sqlite") await takeSnap(page, name);
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
          const result = await new AxeBuilder({ page }).withTags(AXE_TAGS).analyze();
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
