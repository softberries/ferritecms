// A signed `file://` plugin registry for the E2E sites (M6-2, design 14 §10): one plugin,
// `e2e-signed` (the admin-e2e fixture under that name), in versions 0.1.0 and 0.2.0,
// signed by a throwaway publisher key; `index.json` signed by a throwaway registry key
// (`minisign.ts`). Global setup makes one per site and passes it to `serve` through
// `FERRITE_PLUGINS__REGISTRY` and `FERRITE_PLUGINS__REGISTRY_KEY`.

import { createHash } from "node:crypto";
import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { generateKey, signingPayload } from "./minisign";
import { fixtureComponent, fixtureManifest } from "./plugin-fixture";

/** The registry plugin's name. */
export const REGISTRY_PLUGIN = "e2e-signed";
/** Its publisher, as the index names it. */
export const REGISTRY_PUBLISHER = "FerriteCMS E2E";
export const REGISTRY_VERSIONS = ["0.1.0", "0.2.0"] as const;

export interface RegistryFixture {
  /** `file:///…/index.json`. */
  url: string;
  /** The registry key's base64 line (`registry_key`). */
  registryKey: string;
  /** The publisher key id that signs the plugin. */
  publisherKeyId: string;
}

const sha = (b: Buffer | string) => createHash("sha256").update(b).digest("hex");

/**
 * Writes the registry into `dir`; `null` (nothing written) when the fixture component
 * has not been built, so only the trust spec fails, with the build hint.
 */
export function writeRegistry(dir: string): RegistryFixture | null {
  let component: Buffer;
  try {
    component = fixtureComponent();
  } catch {
    return null;
  }
  const registry = generateKey();
  const publisher = generateKey();
  const versions = REGISTRY_VERSIONS.map((version) => {
    const manifest = fixtureManifest(REGISTRY_PLUGIN, { version, publisher: "ferritecms.dev" });
    const folder = join(dir, REGISTRY_PLUGIN, version);
    mkdirSync(folder, { recursive: true });
    writeFileSync(join(folder, "ferrite-plugin.toml"), manifest);
    writeFileSync(join(folder, "e2e_signed.wasm"), component);
    return {
      version,
      world: "ferrite:plugin/plugin@0.1.0",
      published_at: "2026-11-20T10:00:00Z",
      manifest: {
        url: `${REGISTRY_PLUGIN}/${version}/ferrite-plugin.toml`,
        sha256: sha(manifest),
        bytes: Buffer.byteLength(manifest),
      },
      component: {
        url: `${REGISTRY_PLUGIN}/${version}/e2e_signed.wasm`,
        sha256: sha(component),
        bytes: component.length,
      },
      signature: publisher.sign(signingPayload(manifest, component)),
      yanked: null,
    };
  });
  const index = {
    schema: 1,
    name: "E2E registry",
    serial: 1,
    generated_at: "2026-11-20T10:00:00Z",
    publishers: [
      {
        id: "ferritecms.dev",
        name: REGISTRY_PUBLISHER,
        keys: [publisher.publicBase64],
        revoked_keys: [],
      },
    ],
    plugins: [
      {
        name: REGISTRY_PLUGIN,
        publisher: "ferritecms.dev",
        description: "The admin E2E fixture, signed.",
        license: "AGPL-3.0-only",
        versions,
      },
    ],
  };
  const bytes = Buffer.from(JSON.stringify(index, null, 2));
  writeFileSync(join(dir, "index.json"), bytes);
  writeFileSync(join(dir, "index.json.minisig"), registry.sign(bytes));
  if (!existsSync(join(dir, "index.json"))) return null;
  return {
    url: pathToFileURL(join(dir, "index.json")).href,
    registryKey: registry.publicBase64,
    publisherKeyId: publisher.keyId,
  };
}
