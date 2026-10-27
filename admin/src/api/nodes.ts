import { queryOptions } from "@tanstack/react-query";
import { api, unwrap } from "./client";
import { qk } from "./keys";
import type { SchemaNodeDto, SchemaNodeMetaDto } from "./schema";

export type NodeDto = SchemaNodeDto;
/** Every member optional: a node of another version may lack some (rolling deploys). */
export type NodeMetaDto = SchemaNodeMetaDto;

/** `GET /system/nodes` (`manage_system`): every registered node, live and stale. */
export async function listNodes(): Promise<NodeDto[]> {
  const list = await unwrap(api.GET("/api/v1/system/nodes"));
  return list.items;
}

export const nodesQuery = queryOptions({
  queryKey: qk.system.nodes,
  queryFn: listNodes,
  staleTime: 15_000,
});
