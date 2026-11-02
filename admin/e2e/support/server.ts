// Starts a real `ferrite serve` for the E2E run: a fresh data folder, its content (`seed
// demo`, or what a public-themes site is checked on, `projects.ts`), the harness accounts
// (`users.ts`), an SMTP sink, then `serve` on a free loopback port. With SQLite the CLI
// cannot write while `serve` holds the data folder, so everything is written first.

import { execFile, spawn, type ChildProcess } from "node:child_process";
import { createWriteStream, existsSync, mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { createServer, type AddressInfo } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { childEnv } from "./child-env";
import type { Backend, SiteContent } from "./projects";
import { ACCOUNTS } from "./users";
import { startSmtpSink } from "./smtp-sink";

const adminDir = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

/** The binary under test: `FERRITE_BIN`, else the workspace's release build. */
export const FERRITE_BIN = process.env.FERRITE_BIN
  ? resolve(process.env.FERRITE_BIN)
  : resolve(adminDir, "../target/release/ferrite");

export interface Site {
  url: string;
  data: string;
  /** Where the sink writes each received mail (read with `mailbox(dir)`). */
  mailDir: string;
  /** The one-time password `user create` printed for `resetme` (`must_reset`). */
  resetmePassword: string;
  /** The server's stdout and stderr. */
  log: string;
  stop(): Promise<void>;
}

/** The repository's documentation: the `docs` content (`site/build.sh` imports it too). */
const DOCS_CONTENT = resolve(adminDir, "../site/content");

export async function startSite(opts: {
  name: string;
  db?: { url: string; schema: string };
  /** Default `demo`. */
  content?: SiteContent;
}): Promise<Site> {
  const content = opts.content ?? "demo";
  if (!existsSync(FERRITE_BIN)) {
    throw new Error(
      `ferrite binary not found at ${FERRITE_BIN}; build it with ` +
        "`pnpm -C admin build && cargo build --release -p ferrite` (or set FERRITE_BIN)",
    );
  }
  const root = mkdtempSync(join(tmpdir(), `ferrite-e2e-${opts.name}-`));
  const data = join(root, "data");
  const mailDir = join(root, "mail");
  const logDir = join(adminDir, "test-results", "e2e-logs");
  mkdirSync(logDir, { recursive: true });
  const log = join(logDir, `${opts.name}.log`);

  const port = await freePort();
  const url = `http://127.0.0.1:${port}`;
  const sink = await startSmtpSink({ dir: mailDir });
  let child: ChildProcess | undefined;
  try {
    const db = opts.db ? ["--db", opts.db.url, "--schema", opts.db.schema] : [];
    // The docs site's title, as `site/build.sh` gives it.
    const title = content === "docs" ? ["--title", "FerriteCMS"] : [];
    await run(["init", "--data", data, "--url", url, ...db, ...title]);
    if (content === "demo") await run(["seed", "demo", "--data", data]);
    if (content === "business") {
      await run(["seed", "demo", "--data", data, "--profile", "business"]);
    }
    let resetmePassword = "";
    for (const a of ACCOUNTS) {
      if (a.login === "admin" && content !== "docs") {
        // `seed demo` made `admin` without a password.
        await run(
          ["user", "reset-password", "--data", data, "--login", "admin", "--password-stdin"],
          `${a.password}\n`,
        );
        continue;
      }
      const create = [
        "user",
        "create",
        "--data",
        data,
        "--login",
        a.login,
        "--email",
        a.email,
        "--role",
        a.role,
      ];
      if (a.password === null) {
        const out = await run(create);
        const m = /^password: (\S+)$/m.exec(out);
        if (!m?.[1]) throw new Error(`no one-time password in \`user create\` output:\n${out}`);
        resetmePassword = m[1];
      } else {
        await run([...create, "--password-stdin"], `${a.password}\n`);
      }
    }
    if (content === "docs") {
      // After the accounts (`--as admin`). Exit 2 (a skipped file, a missing image, a
      // broken link) fails the start, as it fails `site/build.sh`.
      await run([
        "import",
        "markdown",
        DOCS_CONTENT,
        "--data",
        data,
        "--front-page",
        "--as",
        "admin",
        "--source",
        "ferrite-docs",
      ]);
    }

    child = spawn(FERRITE_BIN, ["serve", "--data", data], {
      env: childEnv(process.env, {
        FERRITE_LOG: process.env.FERRITE_LOG ?? "info",
        NO_COLOR: "1",
        FERRITE_SERVER__LISTEN: `127.0.0.1:${port}`,
        // Each login context sends its own X-Forwarded-For (pre-flight F24).
        FERRITE_SERVER__TRUSTED_PROXIES: '["127.0.0.1/32"]',
        FERRITE_MAIL__SMTP_URL: `smtp://127.0.0.1:${sink.port}`,
        FERRITE_MAIL__FROM: "FerriteCMS <admin@example.test>",
        // The webhooks spec's sink listens on loopback over http (M6-6).
        FERRITE_WEBHOOKS__LOCAL_TARGETS: "true",
      }),
      stdio: ["ignore", "pipe", "pipe"],
    });
    await listening(child, log);
    const proc = child;
    return {
      url,
      data,
      mailDir,
      resetmePassword,
      log,
      stop: async () => {
        await stopChild(proc);
        await sink.stop();
        rmSync(root, { recursive: true, force: true });
      },
    };
  } catch (e) {
    if (child) await stopChild(child);
    await sink.stop();
    rmSync(root, { recursive: true, force: true });
    throw e;
  }
}

function run(args: string[], stdin?: string): Promise<string> {
  return new Promise((resolve, reject) => {
    const p = execFile(
      FERRITE_BIN,
      args,
      { env: childEnv(process.env, { FERRITE_LOG: "warn" }), maxBuffer: 16 << 20 },
      (err, stdout, stderr) => {
        if (err) {
          reject(
            new Error(
              `ferrite ${args.slice(0, 2).join(" ")} failed: ${err.message}\n${stdout}${stderr}`,
            ),
          );
        } else {
          resolve(stdout);
        }
      },
    );
    p.stdin?.end(stdin ?? "");
  });
}

function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const s = createServer();
    s.once("error", reject);
    s.listen(0, "127.0.0.1", () => {
      const { port } = s.address() as AddressInfo;
      s.close(() => resolve(port));
    });
  });
}

/** Resolves once the server prints `Listening on`; rejects if it exits first or in 60 s. */
function listening(child: ChildProcess, logPath: string): Promise<void> {
  const file = createWriteStream(logPath);
  let seen = "";
  return new Promise((resolve, reject) => {
    const timer = setTimeout(
      () => reject(new Error(`ferrite serve did not start within 60 s:\n${seen}`)),
      60_000,
    );
    const onData = (chunk: Buffer) => {
      file.write(chunk);
      if (seen.length < 64_000) seen += chunk.toString("utf8");
      if (seen.includes("Listening on")) {
        clearTimeout(timer);
        resolve();
      }
    };
    child.stdout?.on("data", onData);
    child.stderr?.on("data", onData);
    child.once("exit", (code, signal) => {
      file.end();
      clearTimeout(timer);
      reject(new Error(`ferrite serve exited (${code ?? signal}) before listening:\n${seen}`));
    });
  });
}

async function stopChild(child: ChildProcess): Promise<void> {
  if (child.exitCode !== null || child.signalCode !== null) return;
  const exited = new Promise<void>((r) => child.once("exit", () => r()));
  child.kill("SIGTERM");
  const timer = setTimeout(() => child.kill("SIGKILL"), 15_000);
  await exited;
  clearTimeout(timer);
}

export type { Backend };

/** The `totp` account's enrollment: its secret and the step its confirm code used. */
export interface TotpEnrollment {
  secret: string;
  step: number;
}

/** What global setup tells the workers about a running site (`FERRITE_E2E_SITES`). */
export interface SiteInfo {
  backend: Backend;
  url: string;
  data: string;
  mailDir: string;
  resetmePassword: string;
  totp: TotpEnrollment;
}

const SITES_ENV = "FERRITE_E2E_SITES";

/** Publishes the running sites, by project name (`projects.ts`), to the workers. */
export function publishSites(sites: Record<string, SiteInfo>): void {
  process.env[SITES_ENV] = JSON.stringify(sites);
}

/** The running site of the project `siteKey`, as global setup published it. */
export function siteInfo(siteKey: string): SiteInfo {
  const raw = process.env[SITES_ENV];
  const site = raw ? (JSON.parse(raw) as Record<string, SiteInfo>)[siteKey] : undefined;
  if (!site) throw new Error(`no E2E site for ${siteKey}; global setup did not start one`);
  return site;
}

/** The storage state of a harness account on the site of the project `siteKey`. */
export function authFile(siteKey: string, login: string): string {
  return join(adminDir, "e2e", ".auth", siteKey, `${login}.json`);
}

/** `admin/test-results`, Playwright's output folder. */
export const RESULTS_DIR = join(adminDir, "test-results");

/** State a spec keeps across its retries on one site; emptied by global setup per run. */
export const E2E_STATE_DIR = join(RESULTS_DIR, "e2e-state");
