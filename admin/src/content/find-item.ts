// Naming a content item known by id alone (a relation field's value, a menu entry's
// target, the comments queue's item filter): `GET /content/refs`, which answers every
// type at once with what the caller may read. Lookups asked for in the same task go out
// as one request (at most `MAX_REFS` ids each); an id the answer leaves out is gone or
// unreadable, `null`. Type labels come from the types list, so an item of a type the
// sidebar does not show is still named.

import { queryOptions, useQuery } from "@tanstack/react-query";
import { useCallback, useMemo } from "react";
import { getContentRefs } from "@/api/content";
import { qk } from "@/api/keys";
import type { SchemaContentRefDto } from "@/api/schema";
import { typesQuery } from "@/api/types";
import { useMe } from "@/auth/session";
import { navFor } from "@/shell/nav";

/** Most ids one `GET /content/refs` takes (the server's limit). */
export const MAX_REFS = 200;

export interface TypeRef {
  name: string;
  label: string;
}

/** What naming an item needs; `type` is the type's name. */
export interface ContentTarget {
  id: string;
  title: string;
  status: string;
  type: string;
}

type FetchRefs = (ids: string[]) => Promise<SchemaContentRefDto[]>;

/**
 * A loader that collects the ids asked for in one task and fetches them together
 * (`MAX_REFS` per request), resolving each with its item or `null` when the answer
 * leaves it out. A failed request rejects the lookups of its batch only.
 */
export function createRefLoader(fetchRefs: FetchRefs) {
  let queued = new Map<string, ((r: Promise<SchemaContentRefDto | null>) => void)[]>();
  const flush = () => {
    const batch = queued;
    queued = new Map();
    const ids = [...batch.keys()];
    for (let i = 0; i < ids.length; i += MAX_REFS) {
      const chunk = ids.slice(i, i + MAX_REFS);
      const answer = fetchRefs(chunk).then((items) => new Map(items.map((r) => [r.id, r])));
      for (const id of chunk) {
        const found = answer.then((byId) => byId.get(id) ?? null);
        for (const resolve of batch.get(id) ?? []) resolve(found);
      }
    }
  };
  return (id: string): Promise<SchemaContentRefDto | null> =>
    new Promise((resolve) => {
      // A macrotask, so lookups started by several components' effects in one commit join.
      if (queued.size === 0) setTimeout(flush, 0);
      const waiting = queued.get(id);
      if (waiting) waiting.push(resolve);
      else queued.set(id, [resolve]);
    });
}

const loadRef = createRefLoader(getContentRefs);

/** The query for item `id` (`null`: gone or not readable). */
export function contentTargetQuery(id: string) {
  return queryOptions({
    queryKey: qk.contentRef(id),
    queryFn: async (): Promise<ContentTarget | null> => {
      const r = await loadRef(id);
      return r ? { id: r.id, title: r.title, status: r.status, type: r.type } : null;
    },
    enabled: id !== "",
    staleTime: 60_000,
  });
}

/** A type's label by name (the name itself while the types load or for an unknown one). */
export function useTypeLabel(): (name: string) => string {
  const types = useQuery(typesQuery);
  return useCallback(
    (name: string) => types.data?.find((ty) => ty.name === name)?.label ?? name,
    [types.data],
  );
}

/** The content types the sidebar shows this viewer, in its order (what searches cover). */
export function useNavContentTypes(): TypeRef[] {
  const me = useMe();
  const types = useQuery(typesQuery);
  return useMemo(() => {
    if (!me || !types.data) return [];
    const labels = new Map(types.data.map((ty) => [ty.name, ty.label]));
    return navFor(me, types.data)
      .flatMap((s) => s.items)
      .flatMap((i) =>
        i.contentType ? [{ name: i.contentType, label: labels.get(i.contentType) ?? i.label }] : [],
      );
  }, [me, types.data]);
}
