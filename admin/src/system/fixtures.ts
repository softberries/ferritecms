// Test data for the System screen's component tests.

import type { SchemaAuditDto, SchemaJobDto, SchemaSystemStatusDto } from "@/api/schema";
import type { NodeDto } from "@/api/nodes";

export const NODE_A = "0190a1b2-c3d4-7e5f-8a9b-00000000000a";
export const NODE_B = "0190a1b2-c3d4-7e5f-8a9b-00000000000b";
export const ADMIN_ID = "0190a1b2-c3d4-7e5f-8a9b-0000000000a1";

export function status(over: Partial<SchemaSystemStatusDto> = {}): SchemaSystemStatusDto {
  return {
    version: "0.1.0",
    site_url: "http://localhost:8080",
    mode: "solo",
    database: "sqlite",
    media: "local",
    schema: { verdict: "up_to_date", db_version: 8, binary_version: 8, pending: [] },
    mail: { configured: true, from: "Site <noreply@example.com>" },
    theme: { active: "blog", fallback_nodes: [] },
    jobs: { queued: 2, running: 0, done: 40, dead: 1 },
    nodes: { live: 1, stale: 0 },
    node: { id: NODE_A, started_at: "2026-10-27T08:00:00Z" },
    ...over,
  };
}

export function job(over: Partial<SchemaJobDto> = {}): SchemaJobDto {
  return {
    id: "0190a1b2-c3d4-7e5f-8a9b-000000000001",
    kind: "media.variants",
    status: "dead",
    attempts: 1,
    max_attempts: 5,
    priority: 0,
    created_at: "2026-10-27T08:00:00Z",
    run_at: "2026-10-27T08:00:00Z",
    finished_at: "2026-10-27T08:00:02Z",
    last_error: "image could not be processed: format error",
    locked_by: null,
    handled: true,
    ...over,
  };
}

export function node(over: Partial<NodeDto> = {}): NodeDto {
  return {
    id: NODE_A,
    name: "node-a",
    version: "0.1.0",
    schema_version: 8,
    started_at: "2026-10-27T08:00:00Z",
    heartbeat_at: new Date().toISOString(),
    live: true,
    meta: {},
    ...over,
  };
}

export function audit(over: Partial<SchemaAuditDto> = {}): SchemaAuditDto {
  return {
    id: "0190a1b2-c3d4-7e5f-8a9b-0000000000f1",
    at: "2026-10-27T09:00:00Z",
    actor: { id: ADMIN_ID, login: "admin" },
    action: "settings.update",
    target_kind: "settings",
    target_id: null,
    details: {},
    ...over,
  };
}
