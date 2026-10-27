// Test data for the Plugins screen's unit tests.

import type { PermissionReportDto, PluginDto } from "@/api/plugins";
import type { SchemaPermissionItemDto } from "@/api/schema";

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
    ...over,
  };
}
