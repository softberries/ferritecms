// Starts the E2E sites once per run: one per selected project (`projects.ts`), each with
// its own data folder (and, for `chromium-pg`, its own schema `e2e_<random>` in
// FERRITE_TEST_PG_URL), so engines and backends never share state. Logs the harness
// accounts in over the API and saves their storage states, publishes the sites to the
// workers through the environment, and returns the teardown.

import { execFileSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import { rmSync } from "node:fs";
import { request } from "@playwright/test";
import { writeContactSheet } from "./contact-sheet";
import { selectedProjects, SNAP_PROJECT, type SiteProject } from "./projects";
import {
  authFile,
  publishSites,
  startSite,
  type Site,
  type SiteInfo,
  type TotpEnrollment,
} from "./server";
import { SCREENS_DIR } from "./snap";
import { stepAt, totpCode } from "./totp";
import { ACCOUNTS, account, forwardedFor } from "./users";

export default async function globalSetup(): Promise<() => Promise<void>> {
  const projects = selectedProjects();
  if (projects.length === 0) throw new Error("no E2E project selected");
  const pgUrl = process.env.FERRITE_TEST_PG_URL;
  if (projects.some((p) => p.backend === "pg") && !pgUrl) {
    // CI defines `chromium-pg` without the URL so that a job missing it fails (10 §2).
    throw new Error("chromium-pg is selected but FERRITE_TEST_PG_URL is missing");
  }
  // The contact sheet shows this run's screens only; only `chromium-sqlite` takes them.
  const snaps = projects.some((p) => p.name === SNAP_PROJECT);
  if (snaps) rmSync(SCREENS_DIR, { recursive: true, force: true });
  else console.log(`E2E: no review screenshots or contact sheet outside ${SNAP_PROJECT}`);

  const schemas: string[] = [];
  const started = await Promise.allSettled(
    projects.map((p) => {
      if (p.backend === "sqlite") return startSite({ name: p.name });
      const schema = `e2e_${randomBytes(6).toString("hex")}`;
      schemas.push(schema);
      return startSite({ name: p.name, db: { url: pgUrl ?? "", schema } });
    }),
  );
  const sites: [SiteProject, Site][] = [];
  started.forEach((r, i) => {
    const p = projects[i];
    if (p && r.status === "fulfilled") sites.push([p, r.value]);
  });

  const teardown = async () => {
    await Promise.all(sites.map(([, s]) => s.stop()));
    if (pgUrl) for (const schema of schemas) dropSchema(pgUrl, schema);
    if (snaps) writeContactSheet();
  };

  try {
    const failed = started.find((r) => r.status === "rejected");
    if (failed) throw failed.reason;
    const published: Record<string, SiteInfo> = {};
    for (const [project, site] of sites) {
      await saveStorageStates(project.name, site);
      const totp = await enrollTotp(project.name, site);
      const { url, data, mailDir, resetmePassword } = site;
      published[project.name] = {
        backend: project.backend,
        url,
        data,
        mailDir,
        resetmePassword,
        totp,
      };
    }
    publishSites(published);
  } catch (e) {
    await teardown();
    throw e;
  }
  return teardown;
}

/** One API login per account, each from its own forwarded address (F24). */
async function saveStorageStates(siteKey: string, site: Site): Promise<void> {
  for (const [i, a] of ACCOUNTS.entries()) {
    if (!a.storageState) continue;
    const ctx = await request.newContext({
      baseURL: site.url,
      extraHTTPHeaders: { Origin: site.url, "X-Forwarded-For": forwardedFor(0, i + 1) },
    });
    try {
      const password = a.password ?? site.resetmePassword;
      const res = await ctx.post("/api/v1/auth/login", { data: { login: a.login, password } });
      const body = (await res.json().catch(() => null)) as { status?: string } | null;
      if (res.status() !== 200 || body?.status !== "session") {
        throw new Error(
          `${siteKey}: login of ${a.login} answered ${res.status()} ${JSON.stringify(body)}`,
        );
      }
      await ctx.storageState({ path: authFile(siteKey, a.login) });
    } finally {
      await ctx.dispose();
    }
  }
}

/**
 * Turns TOTP on for the `totp` account over the API (07 §2): log in, `POST
 * /auth/totp/setup`, then `confirm` with a code computed from the returned secret.
 */
async function enrollTotp(siteKey: string, site: Site): Promise<TotpEnrollment> {
  const a = account("totp");
  const ctx = await request.newContext({
    baseURL: site.url,
    extraHTTPHeaders: { Origin: site.url, "X-Forwarded-For": forwardedFor(0, 200) },
  });
  try {
    const login = await ctx.post("/api/v1/auth/login", {
      data: { login: a.login, password: a.password },
    });
    const session = (await login.json().catch(() => null)) as { csrf_token?: string } | null;
    if (login.status() !== 200 || !session?.csrf_token) {
      throw new Error(`${siteKey}: login of totp answered ${login.status()}`);
    }
    const headers = { "X-CSRF-Token": session.csrf_token };
    const setup = await ctx.post("/api/v1/auth/totp/setup", { headers });
    const body = (await setup.json().catch(() => null)) as {
      enrollment?: string;
      secret_base32?: string;
    } | null;
    if (setup.status() !== 200 || !body?.enrollment || !body.secret_base32) {
      throw new Error(`${siteKey}: totp setup answered ${setup.status()}`);
    }
    const step = stepAt();
    const confirm = await ctx.post("/api/v1/auth/totp/confirm", {
      headers,
      data: {
        enrollment: body.enrollment,
        code: totpCode(body.secret_base32, step),
        password: a.password,
      },
    });
    if (!confirm.ok()) {
      throw new Error(
        `${siteKey}: totp confirm answered ${confirm.status()} ${await confirm.text()}`,
      );
    }
    return { secret: body.secret_base32, step };
  } finally {
    await ctx.dispose();
  }
}

function dropSchema(url: string, schema: string): void {
  try {
    execFileSync(
      "psql",
      [url, "-v", "ON_ERROR_STOP=1", "-q", "-c", `DROP SCHEMA IF EXISTS "${schema}" CASCADE`],
      {
        stdio: ["ignore", "ignore", "pipe"],
      },
    );
  } catch (e) {
    const missing = (e as NodeJS.ErrnoException).code === "ENOENT";
    console.warn(
      missing
        ? `psql is not on PATH; drop the E2E schema yourself: ${schema}`
        : `could not drop the E2E schema ${schema}: ${String(e)}`,
    );
  }
}
