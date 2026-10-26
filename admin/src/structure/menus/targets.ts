// What a menu entry links to, for its row: a content item's title and type (looked up in
// the sidebar's types, `content/find-item.ts`), a term's name and taxonomy, or a custom
// link's address. Entries added in this session seed these queries with what Add items
// already knows, so they show without a round trip.

import { queryOptions, useQueries, useQuery } from "@tanstack/react-query";
import { useMemo } from "react";
import { ApiError } from "@/api/problem";
import type { TermDto } from "@/api/taxonomies";
import { taxonomiesQuery } from "@/api/taxonomies";
import { getTerm } from "@/api/terms";
import { contentTargetQuery, useNavContentTypes } from "@/content/find-item";
import type { MenuNode } from "./menu-model";

/** `GET /terms/{id}`; `null` once the server says it is gone. */
export function termTargetQuery(id: string) {
  return queryOptions({
    queryKey: ["term", id],
    queryFn: async (): Promise<TermDto | null> => {
      try {
        return await getTerm(id);
      } catch (e) {
        if (e instanceof ApiError && (e.status === 404 || e.status === 400)) return null;
        throw e;
      }
    },
    staleTime: 60_000,
  });
}

export type Target =
  | { state: "loading" }
  | { state: "missing" }
  | { state: "failed" }
  | { state: "found"; title: string; kind: string; status?: string };

/** Each entry's target by entry id (custom links have none). */
export function useTargets(nodes: readonly MenuNode[]): Map<string, Target> {
  const types = useNavContentTypes();
  const taxonomies = useQuery(taxonomiesQuery);
  const items = useMemo(
    () => [
      ...new Set(
        nodes.flatMap((n) => (n.data.kind === "item" && n.data.targetId ? [n.data.targetId] : [])),
      ),
    ],
    [nodes],
  );
  const terms = useMemo(
    () => [
      ...new Set(
        nodes.flatMap((n) => (n.data.kind === "term" && n.data.targetId ? [n.data.targetId] : [])),
      ),
    ],
    [nodes],
  );
  const itemResults = useQueries({ queries: items.map((id) => contentTargetQuery(id, types)) });
  const termResults = useQueries({ queries: terms.map((id) => termTargetQuery(id)) });

  return useMemo(() => {
    const byTarget = new Map<string, Target>();
    items.forEach((id, i) => {
      const r = itemResults[i];
      if (!r || r.isPending || types.length === 0) byTarget.set(id, { state: "loading" });
      else if (r.isError) byTarget.set(id, { state: "failed" });
      else if (r.data) {
        byTarget.set(id, {
          state: "found",
          title: r.data.title,
          kind: r.data.type.label,
          status: r.data.status,
        });
      } else byTarget.set(id, { state: "missing" });
    });
    terms.forEach((id, i) => {
      const r = termResults[i];
      if (!r || r.isPending) byTarget.set(id, { state: "loading" });
      else if (r.isError) byTarget.set(id, { state: "failed" });
      else if (r.data) {
        const tax = taxonomies.data?.find((x) => x.name === r.data?.taxonomy);
        byTarget.set(id, {
          state: "found",
          title: r.data.name,
          kind: tax?.label ?? r.data.taxonomy,
        });
      } else byTarget.set(id, { state: "missing" });
    });
    const out = new Map<string, Target>();
    for (const n of nodes) {
      const target = n.data.targetId ? byTarget.get(n.data.targetId) : undefined;
      if (target) out.set(n.id, target);
    }
    return out;
  }, [nodes, items, terms, itemResults, termResults, taxonomies.data, types.length]);
}
