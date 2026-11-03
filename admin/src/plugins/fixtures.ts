// Test data for the Plugins screen's unit tests.

import type { PermissionReportDto, PluginDto, TrustDto } from "@/api/plugins";
import type { SchemaInstalledTrustDto, SchemaPermissionItemDto } from "@/api/schema";

/** An unsigned upload's trust part. */
export function trust(over: Partial<TrustDto> = {}): TrustDto {
  return {
    policy: "any",
    signature: "unsigned",
    signer: null,
    owner: { change: "bind_unsigned", current: null, next: null, next_label: null },
    source: { kind: "upload", url: null, version: null },
    warnings: ["unsigned"],
    refusal: null,
    ...over,
  };
}

/** A verified registry signer's trust part. */
export function verifiedTrust(over: Partial<TrustDto> = {}): TrustDto {
  return trust({
    signature: "verified",
    signer: {
      key_id: "432051207B9BA065",
      source: "registry",
      publisher: { id: "ferritecms.dev", name: "FerriteCMS" },
      claimed: "ferritecms.dev",
    },
    owner: {
      change: "bind",
      current: null,
      next: "reg:9F1E000000000000:ferritecms.dev",
      next_label: "FerriteCMS (registry)",
    },
    source: { kind: "registry", url: "https://example.dev/index.json", version: "1.2.0" },
    warnings: [],
    ...over,
  });
}

/** An installed plugin's trust: unsigned by default. */
export function installedTrust(
  over: Partial<SchemaInstalledTrustDto> = {},
): SchemaInstalledTrustDto {
  return { state: "unsigned", owner: null, signer: null, source: null, update: null, ...over };
}

export const NODE_A = "11111111-1111-4111-8111-111111111111";
export const NODE_B = "22222222-2222-4222-8222-222222222222";

export function permission(over: Partial<SchemaPermissionItemDto> = {}): SchemaPermissionItemDto {
  return {
    key: "render.head",
    text: "add tags to the page head",
    detail: [],
    risk: "low",
    high_risk: false,
    ...over,
  };
}

export function report(over: Partial<PermissionReportDto> = {}): PermissionReportDto {
  return {
    plugin: "seo-helper",
    version: "1.2.0",
    artifact: "a".repeat(64),
    fingerprint: "f".repeat(64),
    update: false,
    publisher: "example.com",
    description: "Better titles.",
    license: "MIT",
    homepage: "https://example.com/seo",
    items: [permission()],
    added: [permission()],
    limits: [],
    trust: trust(),
    ...over,
  };
}

export function plugin(over: Partial<PluginDto> = {}): PluginDto {
  return {
    name: "seo-helper",
    version: "1.2.0",
    enabled: true,
    disabled_reason: null,
    publisher: "example.com",
    description: "Better titles.",
    license: "MIT",
    homepage: "https://example.com/seo",
    installed_at: "2026-10-01T10:00:00Z",
    updated_at: "2026-10-02T10:00:00Z",
    nodes: [{ node: NODE_A, name: "node-a", status: "loaded", version: "1.2.0", error: null }],
    report: {
      plugin: "seo-helper",
      version: "1.2.0",
      artifact: "a".repeat(64),
      fingerprint: "f".repeat(64),
      items: [permission()],
      added: [],
      limits: [],
    },
    trust: installedTrust(),
    ...over,
  };
}
