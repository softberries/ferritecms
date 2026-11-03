// /admin/content/$type/$id: an item in the writing surface. The loader reads it with its
// `version` (the `ETag`); a just-created item arrives through the hand-over from `/new`
// and its cached copy, without a second `GET`. Saving is `PATCH` with `If-Match`; the
// side panel and publishing (Task 15), autosave and the conflict dialog (Task 16) and
// revisions (Task 17) build on `useEditorDocument`.
//
// The loader also reads the caller's autosave slot (Task 16): when it differs from the
// item, the screen opens with it and the restore banner. A draft kept over a sign-in
// (`pendingDrafts`, after a 401) wins over the slot: it is newer, and it is written to
// the slot as the screen opens. Discard, Take theirs, Restore and a revision restore
// (Task 17) remount the screen with a new starting point (`onReopen`). The first start
// also reads this browser's local backup of the item (edits made over a kept copy that
// never reached the server, `editor/local-backup.ts`); the screen offers it when it
// differs from both the item and the slot. Images dropped on the body upload through a
// queue that lives here, above the remounted screen (`body-uploads.ts`).
//
// `?release=<id>` (M6-3, design 15 §11.2) is change mode: the loader reads the release and
// the screen edits the item's entry there (its staged change, or the stored item when the
// entry has none), saving through the release; no slot is read or written. A release that
// does not hold the item opens the item as usual.

import { createFileRoute, notFound } from "@tanstack/react-router";
import { useEffect, useMemo, useState } from "react";
import { getContent } from "@/api/content";
import {
  type ReleaseDetail,
  type ReleaseEntry,
  releaseEditable,
  releaseQuery,
} from "@/api/releases";
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
import { useBodyUploads } from "@/editor/body-uploads";
import { EditorScreen, type Reopen, type RevisionNotice } from "@/editor/editor";
import type { RestoreSource } from "@/editor/editor-state";
import { clearHandoff, peekHandoff } from "@/editor/handoff";
import { type Backup, readBackup } from "@/editor/local-backup";
import type { UploadQueue } from "@/media/upload-queue";
import { itemOfChange } from "@/releases/change-mode";

const NOT_EDITED = new Set(["media"]);

interface ItemLoaderData {
  label: string;
  item: SchemaContentDto;
  /** The caller's autosave slot of the item, if any. */
  autosave: SchemaAutosaveDto | null;
  /** Change mode: the release and the item's entry in it. */
  change: { release: ReleaseDetail; entry: ReleaseEntry } | null;
}

interface ItemSearch {
  release?: string;
  check?: string;
}

/** Whether the screen will be editable, so a slot can matter (else none is read). */
const mayHaveSlot = (item: SchemaContentDto) =>
  (item.allowed.edit || item.allowed.stage) && item.status !== "trash";

export const Route = createFileRoute("/_app/content/$type/$id")({
  // `?release=<id>` (M6-3): change mode; `?check=<rule>:<key>` (M6-4): the health
  // report's link to one finding.
  validateSearch: (raw: Record<string, unknown>): ItemSearch => ({
    ...(typeof raw.release === "string" && raw.release !== "" ? { release: raw.release } : {}),
    ...(typeof raw.check === "string" && raw.check.length <= 400 ? { check: raw.check } : {}),
  }),
  // Only the release changes what the loader reads (`check` is the screen's).
  loaderDeps: ({ search }) => ({ release: search.release }),
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
  loader: async ({ context, params, deps }): Promise<ItemLoaderData> => {
    const { queryClient } = context;
    const types = await queryClient.ensureQueryData(typesQuery);
    const type = types.find((ty) => ty.name === params.type);
    if (!type || NOT_EDITED.has(type.name)) throw notFound();
    const key = qk.content.item(params.type, params.id);
    const handed = peekHandoff(params.id)
      ? queryClient.getQueryData<SchemaContentDto>(key)
      : undefined;
    if (handed && !deps.release) {
      return { label: type.label, item: handed, autosave: null, change: null };
    }
    try {
      const item = await queryClient.fetchQuery({
        queryKey: key,
        queryFn: () => getContent(params.type, params.id),
        staleTime: 0,
      });
      if (deps.release) {
        const release = await queryClient.fetchQuery({
          ...releaseQuery(deps.release),
          staleTime: 0,
        });
        const entry = release.entries.find((e) => e.item_id === item.id);
        if (entry) {
          return {
            label: type.label,
            item: itemOfChange(item, entry, releaseEditable(release)),
            autosave: null,
            change: { release, entry },
          };
        }
      }
      // Only an editable screen can use a slot (and only there may the caller read one,
      // so a read-only item asks for nothing that would answer 403).
      const autosave = mayHaveSlot(item) ? await loadAutosave(params.type, params.id) : null;
      return { label: type.label, item, autosave, change: null };
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
  /** This browser's backup of the item: read on the first start, then carried while on offer. */
  backup: Backup | null;
  /** A revision restore remounted the screen (its toast, with Undo). */
  revision: RevisionNotice | null;
  /** The restored draft's Discard remounted the screen (its toast, with Undo). */
  discarded: Draft | null;
  /** Bumped on every reopen: the key that remounts the screen. */
  epoch: number;
  /** The loader result this start belongs to (a new load starts over). */
  from: ItemLoaderData;
}

function ItemEditor() {
  const data = Route.useLoaderData();
  // Another item is another screen (its own drop uploads and reopen state); so is its
  // staged change in a release.
  return <ItemEditorOf key={`${data.item.id}:${data.change?.release.id ?? ""}`} data={data} />;
}

function ItemEditorOf({ data }: { data: ItemLoaderData }) {
  const { type } = Route.useParams();
  const me = useMe();
  // Images dropped on the body: the queue outlives the screen's remounts (every reopen),
  // and a just-created item takes over its `/new` screen's queue (A1).
  const [adopted] = useState(() => peekHandoff(data.item.id)?.uploads?.queue ?? null);
  const uploads = useBodyUploads(adopted);
  const [reopened, setReopened] = useState<Start | null>(null);
  const userId = me?.id;
  // The first start, from the loader: a draft kept over a sign-in, else the slot.
  const initial = useMemo((): Start => {
    const id = data.item.id;
    if (data.change) {
      // Change mode: the release holds the draft; no slot, no backup, no kept draft.
      return {
        item: data.item,
        restore: null,
        kept: null,
        backup: null,
        revision: null,
        discarded: null,
        epoch: 0,
        from: data,
      };
    }
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
    const backup = userId === undefined ? null : readBackup({ item: id, user: userId });
    return {
      item: data.item,
      restore,
      kept: null,
      backup,
      revision: null,
      discarded: null,
      epoch: 0,
      from: data,
    };
  }, [data, userId]);
  const start = reopened?.from === data ? reopened : initial;
  const onReopen = (next: Reopen) => {
    if (!next.item) return;
    setReopened({
      item: next.item,
      restore: next.restore ?? null,
      kept: next.kept ?? null,
      // A backup still on offer stays offered (the screen passes it on).
      backup: next.backup ?? null,
      revision: next.revision ?? null,
      discarded: next.discarded ?? null,
      epoch: start.epoch + 1,
      from: data,
    });
  };
  // The epoch starts at 0 again for a new loader result: no remount when the screen was
  // never reopened (it holds the user's draft; the editor reads its props once), a
  // remount on the fresh load otherwise. The loader runs again only on `invalidate()`,
  // which nothing calls while this screen is up (`shouldReload: false`).
  return (
    <ItemScreen
      key={`${start.item.id}:${start.epoch}`}
      type={type}
      label={data.label}
      start={start}
      uploads={uploads.queue}
      onReopen={onReopen}
      change={data.change}
    />
  );
}

function ItemScreen({
  type,
  label,
  start,
  uploads,
  onReopen,
  change,
}: {
  type: string;
  label: string;
  start: Start;
  uploads: UploadQueue;
  onReopen: (next: Reopen) => void;
  change: ItemLoaderData["change"];
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
      backup={start.backup}
      revision={start.revision}
      discarded={start.discarded}
      uploads={uploads}
      onReopen={onReopen}
      change={change}
    />
  );
}
