// Starts the E2E sites once per run: SQLite always, Postgres when FERRITE_TEST_PG_URL is
// set (schema `e2e_<random>`). Logs the harness accounts in over the API and saves their
// storage states, publishes the sites to the workers through the environment, and
// returns the teardown.

import { execFileSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import { rmSync } from "node:fs";
import { request } from "@playwright/test";
import { writeContactSheet } from "./contact-sheet";
import {
  authFile,
  publishSites,
  startSite,
  type Backend,
  type Site,
  type SiteInfo,
  type TotpEnrollment,
} from "./server";
import { SCREENS_DIR } from "./snap";
import { stepAt, totpCode } from "./totp";
import { ACCOUNTS, account, forwardedFor } from "./users";

export default async function globalSetup(): Promise<() => Promise<void>> {
  // The contact sheet shows this run's screens only.
  rmSync(SCREENS_DIR, { recursive: true, force: true });
  const pgUrl = process.env.FERRITE_TEST_PG_URL;
  const schema = pgUrl ? `e2e_${randomBytes(6).toString("hex")}` : undefined;
  const started = await Promise.allSettled([
    startSite({ name: "sqlite" }),
    ...(pgUrl && schema ? [startSite({ name: "pg", db: { url: pgUrl, schema } })] : []),
  ]);
  const sites: [Backend, Site][] = [];
  started.forEach((r, i) => {
    if (r.status === "fulfilled") sites.push([i === 0 ? "sqlite" : "pg", r.value]);
  });

  const teardown = async () => {
    await Promise.all(sites.map(([, s]) => s.stop()));
    if (pgUrl && schema) dropSchema(pgUrl, schema);
    writeContactSheet();
  };

  try {
    const failed = started.find((r) => r.status === "rejected");
    if (failed) throw failed.reason;
    const published: Partial<Record<Backend, SiteInfo>> = {};
    for (const [backend, site] of sites) {
      await saveStorageStates(backend, site);
      const totp = await enrollTotp(backend, site);
      const { url, data, mailDir, resetmePassword } = site;
      published[backend] = { url, data, mailDir, resetmePassword, totp };
    }
    publishSites(published);
  } catch (e) {
    await teardown();
    throw e;
  }
  return teardown;
}

/** One API login per account, each from its own forwarded address (F24). */
async function saveStorageStates(backend: Backend, site: Site): Promise<void> {
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
          `${backend}: login of ${a.login} answered ${res.status()} ${JSON.stringify(body)}`,
        );
      }
      await ctx.storageState({ path: authFile(backend, a.login) });
    } finally {
      await ctx.dispose();
    }
  }
}

/**
 * Turns TOTP on for the `totp` account over the API (07 §2): log in, `POST
 * /auth/totp/setup`, then `confirm` with a code computed from the returned secret.
 */
async function enrollTotp(backend: Backend, site: Site): Promise<TotpEnrollment> {
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
      throw new Error(`${backend}: login of totp answered ${login.status()}`);
    }
    const headers = { "X-CSRF-Token": session.csrf_token };
    const setup = await ctx.post("/api/v1/auth/totp/setup", { headers });
    const body = (await setup.json().catch(() => null)) as {
      enrollment?: string;
      secret_base32?: string;
    } | null;
    if (setup.status() !== 200 || !body?.enrollment || !body.secret_base32) {
      throw new Error(`${backend}: totp setup answered ${setup.status()}`);
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
        `${backend}: totp confirm answered ${confirm.status()} ${await confirm.text()}`,
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
