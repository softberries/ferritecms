import { queryOptions } from "@tanstack/react-query";
import { api, unwrap } from "./client";
import { qk } from "./keys";
import type {
  SchemaNodeDto,
  SchemaNodeMetaDto,
  SchemaNodePluginDto,
  SchemaNodeThemeDto,
} from "./schema";

export type NodeDto = SchemaNodeDto;
/** Every member optional: a node of another version may lack some (rolling deploys). */
export type NodeMetaDto = SchemaNodeMetaDto;

/** `GET /system/nodes` (`manage_system`): every registered node, live and stale. */
export async function listNodes(): Promise<NodeDto[]> {
  const list = await unwrap(api.GET("/api/v1/system/nodes"));
  return list.items.map((n) => ({ ...n, meta: nodeMeta(n.meta) }));
}

type Obj = Record<string, unknown>;
const isObj = (v: unknown): v is Obj => typeof v === "object" && v !== null && !Array.isArray(v);
const str = (v: unknown): string | undefined => (typeof v === "string" ? v : undefined);

/** Copies the members of `from` named in `keys` whose value passes `ok`. */
function pick<T>(from: Obj, keys: readonly string[], ok: (v: unknown) => boolean): Partial<T> {
  const out: Obj = {};
  for (const k of keys) if (ok(from[k])) out[k] = from[k];
  return out as Partial<T>;
}

/**
 * A node's `meta` as the screens may read it (H-Y2): the server scrubs its strings but
 * not its shape, and a node of another version may send other shapes. Every member of
 * the wrong type is dropped (a `null` plugin entry, `job_kinds` that is not a list of
 * strings, a theme that is not an object), so nothing downstream can throw on it.
 */
export function nodeMeta(raw: unknown): NodeMetaDto {
  if (!isObj(raw)) return {};
  const meta: NodeMetaDto = pick<NodeMetaDto>(
    raw,
    ["mode", "site_id", "site_url", "plugins_error", "permalinks_error"],
    (v) => typeof v === "string",
  );
  if (typeof raw.clock_skew_ms === "number" && Number.isFinite(raw.clock_skew_ms)) {
    meta.clock_skew_ms = raw.clock_skew_ms;
  }
  if (Array.isArray(raw.job_kinds)) {
    meta.job_kinds = raw.job_kinds.filter((k): k is string => typeof k === "string");
  }
  if (isObj(raw.theme)) {
    const theme: SchemaNodeThemeDto = pick<SchemaNodeThemeDto>(
      raw.theme,
      ["name", "hash", "error"],
      (v) => typeof v === "string",
    );
    if (typeof raw.theme.fallback === "boolean") theme.fallback = raw.theme.fallback;
    const origin = raw.theme.origin;
    if (typeof origin === "string") theme.origin = origin;
    else if (isObj(origin) && typeof origin.artifact === "string") {
      theme.origin = { artifact: origin.artifact };
    }
    meta.theme = theme;
  }
  if (isObj(raw.plugins)) {
    const plugins: Record<string, SchemaNodePluginDto> = {};
    for (const [name, p] of Object.entries(raw.plugins)) {
      if (!isObj(p)) continue;
      plugins[name] = pick<SchemaNodePluginDto>(
        p,
        ["version", "hash", "status", "error"],
        (v) => str(v) !== undefined,
      );
    }
    meta.plugins = plugins;
  }
  return meta;
}

export const nodesQuery = queryOptions({
  queryKey: qk.system.nodes,
  queryFn: listNodes,
  staleTime: 15_000,
});
