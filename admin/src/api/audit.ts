import { queryOptions } from "@tanstack/react-query";
import { api, unwrap } from "./client";
import { type AuditFilters, qk } from "./keys";
import type { SchemaAuditDto, SchemaAuditList } from "./schema";

export type AuditDto = SchemaAuditDto;

/** Audit rows a page (constraints: exact values). */
export const AUDIT_PAGE = 50;

/**
 * `GET /system/audit` (`manage_system`): one page, newest first. `action` is an exact
 * action or a prefix ending in `.` (`user.`).
 */
export function listAudit(
  query: AuditFilters & { cursor?: string; limit?: number } = {},
  options: { signal?: AbortSignal } = {},
): Promise<SchemaAuditList> {
  return unwrap(
    api.GET("/api/v1/system/audit", {
      params: { query },
      ...(options.signal ? { signal: options.signal } : {}),
    }),
  );
}

export function auditQuery(filters: AuditFilters) {
  return queryOptions({
    queryKey: qk.system.audit(filters),
    queryFn: ({ signal }) => listAudit({ ...filters, limit: AUDIT_PAGE }, { signal }),
    staleTime: 30_000,
  });
}
