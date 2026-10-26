// /admin/content/$type/$id: an item in the writing surface. The loader reads it with its
// `version` (the `ETag`); a just-created item arrives through the hand-over from `/new`
// and its cached copy, without a second `GET`. Saving is `PATCH` with `If-Match`; the
// side panel and publishing (Task 15), autosave and the conflict dialog (Task 16) and
// revisions (Task 17) build on `useEditorDocument`.
//
// The loader also reads the caller's autosave slot (Task 16): when it differs from the
// item, the screen opens with it and the restore banner. A draft kept over a sign-in
// (`pendingDrafts`, after a 401) wins over the slot: it is newer, and it is written to
// the slot as the screen opens. Discard, Take theirs and Restore remount the screen with
// a new starting point (`onReopen`).

import { createFileRoute, notFound } from "@tanstack/react-router";
import { useEffect, useMemo, useState } from "react";
import { getContent } from "@/api/content";
import { qk } from "@/api/keys";
import { ApiError } from "@/api/problem";
import type { SchemaAutosaveDto, SchemaContentDto } from "@/api/schema";
import { typesQuery } from "@/api/types";
import { useMe } from "@/auth/session";
import {
  type Draft,
  draftOfAutosave,
  dropPendingDraft,
  loadAutosave,
  peekPendingDraft,
} from "@/editor/autosave";
import { EditorScreen, type Reopen } from "@/editor/editor";
import { type RestoreSource, clearHandoff, peekHandoff } from "@/editor/editor-state";

const NOT_EDITED = new Set(["media"]);

interface ItemLoaderData {
  label: string;
  item: SchemaContentDto;
  /** The caller's autosave slot of the item, if any. */
  autosave: SchemaAutosaveDto | null;
}

/** Whether the screen will be editable, so a slot can matter (else none is read). */
const mayHaveSlot = (item: SchemaContentDto) => item.allowed.edit && item.status !== "trash";

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
    if (handed) return { label: type.label, item: handed, autosave: null };
    try {
      const item = await queryClient.fetchQuery({
        queryKey: key,
        queryFn: () => getContent(params.type, params.id),
        staleTime: 0,
      });
      // Only an editable screen can use a slot (and only there may the caller read one,
      // so a read-only item asks for nothing that would answer 403).
      const autosave = mayHaveSlot(item) ? await loadAutosave(params.type, params.id) : null;
      return { label: type.label, item, autosave };
    } catch (e) {
      if (e instanceof ApiError && (e.status === 404 || e.status === 400)) throw notFound();
      throw e;
    }
  },
  component: ItemEditor,
});

interface Start {
  item: SchemaContentDto;
  restore: RestoreSource | null;
  kept: Draft | null;
  /** Bumped on every reopen: the key that remounts the screen. */
  epoch: number;
  /** The loader result this start belongs to (a new load starts over). */
  from: ItemLoaderData;
}

function ItemEditor() {
  const { type } = Route.useParams();
  const data = Route.useLoaderData();
  const me = useMe();
  const [reopened, setReopened] = useState<Start | null>(null);
  const userId = me?.id;
  // The first start, from the loader: a draft kept over a sign-in, else the slot.
  const initial = useMemo((): Start => {
    const id = data.item.id;
    const pending = peekPendingDraft(id, userId);
    const restore: RestoreSource | null = pending
      ? {
          draft: pending.draft,
          at: pending.at,
          flush: true,
          onStored: () => dropPendingDraft(id, pending),
        }
      : data.autosave
        ? { draft: draftOfAutosave(data.autosave), at: data.autosave.saved_at }
        : null;
    return { item: data.item, restore, kept: null, epoch: 0, from: data };
  }, [data, userId]);
  const start = reopened?.from === data ? reopened : initial;
  const onReopen = (next: Reopen) => {
    if (!next.item) return;
    setReopened({
      item: next.item,
      restore: next.restore ?? null,
      kept: next.kept ?? null,
      epoch: start.epoch + 1,
      from: data,
    });
  };
  return (
    <ItemScreen
      key={`${start.item.id}:${start.epoch}`}
      type={type}
      label={data.label}
      start={start}
      onReopen={onReopen}
    />
  );
}

function ItemScreen({
  type,
  label,
  start,
  onReopen,
}: {
  type: string;
  label: string;
  start: Start;
  onReopen: (next: Reopen) => void;
}) {
  const { item } = start;
  // Read once per mount, dropped once mounted (StrictMode may run the initializer twice).
  const [handoff] = useState(() => (start.epoch === 0 ? peekHandoff(item.id) : null));
  useEffect(() => clearHandoff(item.id), [item.id]);
  return (
    <EditorScreen
      type={type}
      typeLabel={label}
      item={item}
      handoff={handoff}
      restore={start.restore}
      kept={start.kept}
      onReopen={onReopen}
    />
  );
}
