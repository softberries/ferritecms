// The admin E2E fixture plugin (`plugins/examples/admin-e2e`, built for `wasm32-wasip2`;
// CI builds it before the E2E job): its component, and its manifest under another name
// (tests install it under unique names, `uid()`), optionally with other `[plugin]`
// fields (the hostile-manifest test).

import { existsSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../../..");
const MANIFEST = join(root, "plugins/examples/admin-e2e/ferrite-plugin.toml");
const COMPONENT = join(root, "plugins/target/wasm32-wasip2/release/admin_e2e.wasm");

/** The built component; a clear error when it has not been built. */
export function fixtureComponent(): Buffer {
  if (!existsSync(COMPONENT)) {
    throw new Error(
      `${COMPONENT} not found; build it with ` +
        "`cargo build --manifest-path plugins/Cargo.toml --target wasm32-wasip2 --release -p admin-e2e`",
    );
  }
  return readFileSync(COMPONENT);
}

/** TOML basic string. */
const quote = (s: string) => JSON.stringify(s);

/**
 * The fixture's manifest with `name` and any other `[plugin]` string fields replaced
 * (`version`, `description`, `publisher`, `license`, `homepage`).
 */
export function fixtureManifest(name: string, fields: Record<string, string> = {}): string {
  let text = readFileSync(MANIFEST, "utf8");
  for (const [key, value] of Object.entries({ ...fields, name })) {
    const line = new RegExp(`^${key}\\s*=.*$`, "m");
    if (!line.test(text)) throw new Error(`no ${key} in the fixture manifest`);
    text = text.replace(line, `${key} = ${quote(value)}`);
  }
  return text;
}

/** The two files of a plugin upload in a new temporary folder, for `setInputFiles`. */
export function fixtureFiles(manifest: string): { manifest: string; component: string } {
  const dir = mkdtempSync(join(tmpdir(), "ferrite-e2e-plugin-"));
  const files = {
    manifest: join(dir, "ferrite-plugin.toml"),
    component: join(dir, "admin_e2e.wasm"),
  };
  writeFileSync(files.manifest, manifest);
  writeFileSync(files.component, fixtureComponent());
  return files;
}
