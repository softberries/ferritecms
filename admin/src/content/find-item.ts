// Finding a content item by id alone (a relation field's value, a menu entry's target):
// summaries carry no type, so the item is looked up in each content type the sidebar
// shows, in its order, and the first that has it wins (P37). A type that answers 404 or
// 400 does not have it; one that is gone or unreadable everywhere is `null`.

import { queryOptions, useQuery } from "@tanstack/react-query";
import { useMemo } from "react";
import { getContent } from "@/api/content";
import { ApiError } from "@/api/problem";
import { typesQuery } from "@/api/types";
import { useMe } from "@/auth/session";
import { navFor } from "@/shell/nav";

export interface TypeRef {
  name: string;
  label: string;
}

export interface ContentTarget {
  id: string;
  title: string;
  status: string;
  type: TypeRef;
}

/** The content types the sidebar shows this viewer, in its order. */
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

/** The query for item `id` in the first of `types` that has it (`null`: none does). */
export function contentTargetQuery(id: string, types: readonly TypeRef[]) {
  return queryOptions({
    queryKey: ["content", "relation", id, types.map((t) => t.name).join(",")],
    queryFn: async (): Promise<ContentTarget | null> => {
      for (const type of types) {
        try {
          const item = await getContent(type.name, id);
          return { id: item.id, title: item.title, status: item.status, type };
        } catch (e) {
          if (e instanceof ApiError && (e.status === 404 || e.status === 400)) continue;
          throw e;
        }
      }
      return null;
    },
    enabled: id !== "" && types.length > 0,
    staleTime: 60_000,
  });
}
