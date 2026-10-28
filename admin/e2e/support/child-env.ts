// The environment the harness gives a `ferrite` process it starts.

/**
 * The harness's own variables. They share the `FERRITE_` prefix, which `ferrite` reads as
 * configuration overrides and refuses when unknown ("env:FERRITE_BIN: unknown key"), so
 * they are left out of a child's environment.
 */
const HARNESS_ONLY = ["FERRITE_BIN", "FERRITE_E2E_SITES"] as const;

/** `env` without the harness's own variables, plus `extra`. */
export function childEnv(
  env: NodeJS.ProcessEnv,
  extra: Record<string, string> = {},
): NodeJS.ProcessEnv {
  const out: NodeJS.ProcessEnv = { ...env, ...extra };
  for (const name of HARNESS_ONLY) delete out[name];
  return out;
}
