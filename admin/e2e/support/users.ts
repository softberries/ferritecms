// The harness accounts every E2E site gets (`server.ts` creates them, `global-setup.ts`
// logs most of them in once and saves their storage states).
//
// Shared sessions are never logged out or reset (pre-flight F23): a confirmed password
// reset ends every session of the account, so the reset flows use `resetuser`, which has
// no storage state; a test that logs out or lets a session expire logs in a fresh session
// of its own (`fixtures.ts` `freshSession`). `author2` is a second author for tests that
// compare one author's counts with a list (F35).

/** A harness account, by its login. */
export type Role =
  "admin" | "editor" | "author" | "author2" | "contributor" | "resetuser" | "resetme";

export interface Account {
  login: Role;
  email: string;
  /** The role given to `ferrite user create --role` (`admin` comes from `seed demo`). */
  role: "admin" | "editor" | "author" | "contributor";
  /**
   * The password set with `--password-stdin`; `null` for `resetme`, which is created
   * without one, so it is `must_reset` and its one-time password is parsed from the
   * command's output (`Site.resetmePassword`).
   */
  password: string | null;
  /** Whether global setup saves a storage state (`e2e/.auth/<backend>/<login>.json`). */
  storageState: boolean;
}

// Test-only passwords: at least 12 characters (the server's policy).
export const ACCOUNTS: readonly Account[] = [
  {
    login: "admin",
    email: "admin@example.test",
    role: "admin",
    password: "e2e-admin-pass-0001",
    storageState: true,
  },
  {
    login: "editor",
    email: "editor@example.test",
    role: "editor",
    password: "e2e-editor-pass-0002",
    storageState: true,
  },
  {
    login: "author",
    email: "author@example.test",
    role: "author",
    password: "e2e-author-pass-0003",
    storageState: true,
  },
  {
    login: "author2",
    email: "author2@example.test",
    role: "author",
    password: "e2e-author2-pass-0004",
    storageState: true,
  },
  {
    login: "contributor",
    email: "contributor@example.test",
    role: "contributor",
    password: "e2e-contributor-pass-0005",
    storageState: true,
  },
  {
    login: "resetuser",
    email: "resetuser@example.test",
    role: "author",
    password: "e2e-resetuser-pass-0006",
    storageState: false,
  },
  {
    login: "resetme",
    email: "resetme@example.test",
    role: "author",
    password: null,
    storageState: true,
  },
];

export function account(login: Role): Account {
  const a = ACCOUNTS.find((x) => x.login === login);
  if (!a) throw new Error(`unknown harness account ${login}`);
  return a;
}

/**
 * The `X-Forwarded-For` address of one login context. Every login counts against the
 * per-IP limit (10/min, 07 §2), so each context sends its own address and the sites trust
 * 127.0.0.1 as a proxy (pre-flight F24). Global setup uses spec 0; each spec file that
 * logs in picks its own `spec` number (1..255) and numbers its contexts with `n`.
 */
export function forwardedFor(spec: number, n: number): string {
  if (!Number.isInteger(spec) || spec < 0 || spec > 255) throw new Error(`bad spec ${spec}`);
  if (!Number.isInteger(n) || n < 1 || n > 254) throw new Error(`bad n ${n}`);
  return `10.0.${spec}.${n}`;
}
