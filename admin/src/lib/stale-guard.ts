export type StaleChoice = "reload" | "overwrite" | "cancel";

export type StaleOutcome<T> = { proceed: true } | { proceed: false; reloaded?: T };

/** Equality by JSON with object keys sorted, so key order does not matter. */
function canonical(value: unknown): string {
  return JSON.stringify(value, (_key, v: unknown) => {
    if (typeof v === "object" && v !== null && !Array.isArray(v)) {
      return Object.fromEntries(
        Object.entries(v as Record<string, unknown>).sort(([a], [b]) =>
          a < b ? -1 : a > b ? 1 : 0,
        ),
      );
    }
    return v;
  });
}

export function stableEqual<T>(a: T, b: T): boolean {
  return canonical(a) === canonical(b);
}

/**
 * Guard for a last-write-wins resource (types, taxonomies, terms, menus, redirects;
 * 07 §1 P11): re-reads it before a write and, when it differs from `base` (the copy the
 * form started from), asks. `reload` hands back the fresh copy and the write does not
 * happen; `overwrite` proceeds; `cancel` stops. The gap between this check and the write
 * is accepted (P31).
 */
export async function staleGuard<T>(opts: {
  reread: () => Promise<T>;
  base: T;
  equal?: (a: T, b: T) => boolean;
  ask: (theirs: T) => Promise<StaleChoice>;
}): Promise<StaleOutcome<T>> {
  const theirs = await opts.reread();
  if ((opts.equal ?? stableEqual)(opts.base, theirs)) return { proceed: true };
  const choice = await opts.ask(theirs);
  if (choice === "overwrite") return { proceed: true };
  if (choice === "reload") return { proceed: false, reloaded: theirs };
  return { proceed: false };
}
