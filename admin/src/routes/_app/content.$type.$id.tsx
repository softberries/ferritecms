// /admin/content/$type/$id: an item in the writing surface. The loader reads it with its
// `version` (the `ETag`); a just-created item arrives through the hand-over from `/new`
// and its cached copy, without a second `GET`. Saving is `PATCH` with `If-Match`; the
// side panel and publishing (Task 15), autosave and the conflict dialog (Task 16) and
// revisions (Task 17) build on `useEditorDocument`.

import { createFileRoute, notFound } from "@tanstack/react-router";
import { useEffect, useState } from "react";
import { getContent } from "@/api/content";
import { qk } from "@/api/keys";
import { ApiError } from "@/api/problem";
import type { SchemaContentDto } from "@/api/schema";
import { typesQuery } from "@/api/types";
import { EditorScreen } from "@/editor/editor";
import { clearHandoff, peekHandoff } from "@/editor/editor-state";

const NOT_EDITED = new Set(["media"]);

interface ItemLoaderData {
  label: string;
  item: SchemaContentDto;
}

export const Route = createFileRoute("/_app/content/$type/$id")({
  staticData: {
    crumb: (t, data) => {
      const item = (data as ItemLoaderData | undefined)?.item;
      return item ? item.title || t("crumbs.untitled") : t("crumbs.content");
    },
    transitionKey: "editor",
  },
  // Always the server's current copy when the screen opens (never a stale cache).
  staleTime: 0,
  gcTime: 0,
  shouldReload: false,
  loader: async ({ context, params }): Promise<ItemLoaderData> => {
    const { queryClient } = context;
    const types = await queryClient.ensureQueryData(typesQuery);
    const type = types.find((ty) => ty.name === params.type);
    if (!type || NOT_EDITED.has(type.name)) throw notFound();
    const key = qk.content.item(params.type, params.id);
    const handed = peekHandoff(params.id)
      ? queryClient.getQueryData<SchemaContentDto>(key)
      : undefined;
    if (handed) return { label: type.label, item: handed };
    try {
      const item = await queryClient.fetchQuery({
        queryKey: key,
        queryFn: () => getContent(params.type, params.id),
        staleTime: 0,
      });
      return { label: type.label, item };
    } catch (e) {
      if (e instanceof ApiError && (e.status === 404 || e.status === 400)) throw notFound();
      throw e;
    }
  },
  component: ItemEditor,
});

function ItemEditor() {
  const { type } = Route.useParams();
  const { label, item } = Route.useLoaderData();
  return <ItemScreen key={item.id} type={type} label={label} item={item} />;
}

function ItemScreen({
  type,
  label,
  item,
}: {
  type: string;
  label: string;
  item: SchemaContentDto;
}) {
  // Read once per mount, dropped once mounted (StrictMode may run the initializer twice).
  const [handoff] = useState(() => peekHandoff(item.id));
  useEffect(() => clearHandoff(item.id), [item.id]);
  return <EditorScreen type={type} typeLabel={label} item={item} handoff={handoff} />;
}
