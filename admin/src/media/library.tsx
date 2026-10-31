// The media library (`/admin/media`, 07 §3): a page header (Media, the count of loaded
// items, **Upload**), a filter bar (search on `/`, kind chips, Mine, Trash, grid/list),
// the grid or the list with infinite paging by `next_cursor` (50 a page), bulk trash
// from the selection bar with Undo, the Trash view with Restore and Delete permanently,
// the detail sheet for `?id=` and the upload tray. The whole page takes dropped files;
// new uploads appear at the top with the layout animation. Actions show by capability
// (`rights.ts`); the server decides every call. The filter bar is `library-toolbar.tsx`;
// the empty states, "Load more" and the confirm are `library-parts.tsx`.

import {
  keepPreviousData,
  useInfiniteQuery,
  useQueryClient,
  type InfiniteData,
} from "@tanstack/react-query";
import { UploadIcon } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { qk } from "@/api/keys";
import { type MediaDto, listMedia } from "@/api/media";
import { ApiError } from "@/api/problem";
import type { SchemaMediaList } from "@/api/schema";
import type { MeDto } from "@/auth/session";
import type { RowSelectionState } from "@/components/kit/data-table-features";
import { DropZone } from "@/components/kit/drop-zone";
import { ErrorState } from "@/components/kit/error-state";
import { ForbiddenState } from "@/components/kit/forbidden-state";
import { PageHeader } from "@/components/kit/page-header";
import { Button } from "@/components/ui/button";
import { BulkBar } from "@/content/bulk-bar";
import { trashUndoable } from "@/content/bulk";
import { uiLocale } from "@/lib/locale";
import { useShortcut } from "@/lib/shortcuts";
import { type MediaAction, useMediaActions } from "./media-actions";
import { MediaDetail } from "./media-detail";
import { MediaGrid } from "./media-grid";
import {
  type MediaSearch,
  type MediaView,
  effectiveView,
  mediaQuery,
  rememberView,
} from "./media-search";
import { MediaTable } from "./media-table";
import { type MediaBulkItem, bulkItem, canUpload } from "./rights";
import { UploadTray } from "./upload-tray";
import { useUploadQueue } from "./use-upload-queue";
import { useUploadRequest } from "./upload-intent";
import { type Confirm, ConfirmDialog, LibraryEmpty, LoadMore } from "./library-parts";
import { Toolbar } from "./library-toolbar";

export interface MediaLibraryProps {
  search: MediaSearch;
  me: MeDto;
  /** Replaces members of the URL's search (`undefined` removes one). */
  setSearch: (patch: Partial<Record<keyof MediaSearch, unknown>>) => void;
}

type Pages = InfiniteData<SchemaMediaList, string>;

/** Whether a fresh upload belongs in the list `filters` shows (newest first, no search). */
function showsUpload(filters: ReturnType<typeof mediaQuery>, media: MediaDto): boolean {
  if (filters.trash || filters.q !== undefined) return false;
  if (filters.kind === undefined) return true;
  const family = media.mime.split("/")[0];
  return filters.kind === "document"
    ? !["image", "video", "audio"].includes(family ?? "")
    : family === filters.kind;
}

export function MediaLibrary({ search, me, setSearch }: MediaLibraryProps) {
  const { t } = useTranslation("media");
  const qc = useQueryClient();
  const uploads = canUpload(me);
  const trash = search.trash === true;
  const [view, setView] = useState<MediaView>(() => effectiveView(search));
  const shownView = search.view ?? view;

  const filters = useMemo(() => mediaQuery(search, me.id), [search, me.id]);
  const listKey = qk.media.infinite(filters);
  const list = useInfiniteQuery({
    queryKey: listKey,
    queryFn: ({ pageParam, signal }) =>
      listMedia(pageParam ? { ...filters, cursor: pageParam } : filters, { signal }),
    initialPageParam: "",
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    placeholderData: keepPreviousData,
  });
  const items = useMemo(() => {
    const seen = new Map<string, MediaDto>();
    for (const page of list.data?.pages ?? []) {
      for (const item of page.items) if (!seen.has(item.id)) seen.set(item.id, item);
    }
    return [...seen.values()];
  }, [list.data]);

  // Uploads land at the top of the list on screen at once; the lists refetch when the
  // queue is idle again.
  const { queue, entries } = useUploadQueue((media) => {
    qc.setQueryData<Pages>(listKey, (data) => {
      if (!data || !showsUpload(filters, media)) return data;
      const [first, ...rest] = data.pages;
      if (!first) return data;
      return { ...data, pages: [{ ...first, items: [media, ...first.items] }, ...rest] };
    });
  });
  const active = entries.some((e) => e.state === "queued" || e.state === "uploading");
  const wasActive = useRef(false);
  useEffect(() => {
    if (wasActive.current && !active) void qc.invalidateQueries({ queryKey: qk.media.lists });
    wasActive.current = active;
  }, [active, qc]);

  const input = useRef<HTMLInputElement>(null);
  const pick = () => input.current?.click();
  useUploadRequest(pick, uploads);

  // Selection belongs to one list: other filters start empty.
  const scope = JSON.stringify(filters);
  const [selection, setSelection] = useState<{ scope: string; ids: RowSelectionState }>({
    scope,
    ids: {},
  });
  const selectedIds = selection.scope === scope ? selection.ids : {};
  const selected: MediaBulkItem[] = items
    .filter((i) => selectedIds[i.id])
    .map((m) => bulkItem(me, m));
  const setSelected = (ids: RowSelectionState) => setSelection({ scope, ids });
  const clearSelection = () => setSelection({ scope, ids: {} });

  const actions = useMediaActions({
    onDone: (ok) =>
      setSelection((s) => ({
        scope: s.scope,
        ids: Object.fromEntries(Object.entries(s.ids).filter(([id]) => !ok.includes(id))),
      })),
  });
  const [confirm, setConfirm] = useState<Confirm | null>(null);
  const run = (action: MediaAction, targets: MediaBulkItem[]) => {
    if (action === "delete") {
      setConfirm({ kind: "delete", items: targets.filter((i) => i.allowed.delete) });
      return;
    }
    if (action === "trash" && !trashUndoable(targets)) {
      setConfirm({ kind: "trash", items: targets.filter((i) => i.allowed.trash) });
      return;
    }
    actions.run(action, targets);
  };

  useShortcut({
    id: "media.trashSelection",
    keys: "#",
    label: t("shortcuts.trashSelection"),
    group: t("shortcuts.group"),
    when: () => selected.some((i) => i.allowed.trash),
    run: () => run("trash", selected),
  });

  const open = (m: MediaDto) => setSearch({ id: m.id });
  const chooseView = (v: MediaView) => {
    rememberView(v);
    setView(v);
    setSearch({ view: v === "grid" ? undefined : v });
  };

  const number = new Intl.NumberFormat(uiLocale());
  const count = list.data ? `${number.format(items.length)}${list.hasNextPage ? "+" : ""}` : null;
  // Refused by the server: the state alone, as other screens show it (no header, upload
  // or filters that could only fail again). Uploads already running keep their tray and
  // its leave guard.
  if (list.error instanceof ApiError && list.error.status === 403) {
    return (
      <>
        <div className="mx-auto w-full max-w-7xl">
          <ForbiddenState />
        </div>
        <UploadTray queue={queue} entries={entries} raised={false} onOpen={open} />
      </>
    );
  }

  const empty = (
    <LibraryEmpty
      search={search}
      uploads={uploads}
      onUpload={pick}
      onClear={() => setSearch({ q: undefined, kind: undefined, mine: undefined })}
    />
  );
  const footer =
    items.length > 0 ? (
      <LoadMore
        hasNext={list.hasNextPage}
        loadingNext={list.isFetchingNextPage}
        shown={items.length}
        onMore={() => {
          if (!list.isFetchingNextPage) void list.fetchNextPage();
        }}
      />
    ) : null;
  // The previous answer stays up at full colour while a new search or kind loads.
  const stale = list.isPlaceholderData;

  const page = (
    <>
      <PageHeader
        title={
          <span className="flex items-baseline gap-2.5">
            {trash ? t("trash.title") : t("title")}
            {count !== null ? (
              <span className="text-lg font-normal text-muted-foreground tabular-nums">
                {count}
              </span>
            ) : null}
          </span>
        }
        description={trash ? t("trash.description") : undefined}
        actions={
          uploads && !trash ? (
            <Button onClick={pick}>
              <UploadIcon data-icon="inline-start" />
              {t("upload")}
            </Button>
          ) : null
        }
      />
      {uploads ? (
        <input
          ref={input}
          type="file"
          multiple
          hidden
          tabIndex={-1}
          data-slot="media-file-input"
          onChange={(e) => {
            const files = Array.from(e.currentTarget.files ?? []);
            if (files.length > 0) queue.add(files);
            e.currentTarget.value = "";
          }}
        />
      ) : null}
      <Toolbar search={search} view={shownView} onChange={setSearch} onView={chooseView} />
      {list.isError && !list.data ? (
        <ErrorState
          error={list.error}
          retry={() => {
            void list.refetch();
          }}
        />
      ) : shownView === "grid" ? (
        <MediaGrid
          items={items}
          label={t("grid.label")}
          loading={list.isPending}
          empty={empty}
          footer={footer}
          selection={{ state: selectedIds, onChange: setSelected }}
          onOpen={open}
          stale={stale}
        />
      ) : (
        <MediaTable
          items={items}
          label={t("grid.label")}
          loading={list.isPending}
          empty={empty}
          footer={footer}
          selection={{ state: selectedIds, onChange: setSelected }}
          onOpen={open}
          stale={stale}
        />
      )}
      {/* Room for the floating bars, so they never hide the last row. */}
      {selected.length > 0 || entries.length > 0 ? (
        <div aria-hidden="true" className="h-14" />
      ) : null}
    </>
  );

  return (
    <>
      {uploads ? (
        <DropZone
          chooser={false}
          onFiles={(files) => queue.add(files)}
          className="mx-auto w-full max-w-7xl gap-5"
        >
          {page}
        </DropZone>
      ) : (
        <div className="mx-auto flex w-full max-w-7xl flex-col gap-5">{page}</div>
      )}
      <BulkBar
        selected={selected}
        pending={actions.pending}
        onAction={(action) => {
          if (action !== "publish") run(action, selected);
        }}
        onClear={clearSelection}
      />
      <UploadTray queue={queue} entries={entries} raised={selected.length > 0} onOpen={open} />
      {search.id ? (
        <MediaDetail
          key={search.id}
          id={search.id}
          me={me}
          onClose={() => setSearch({ id: undefined })}
          onAction={(action, media) => {
            setSearch({ id: undefined });
            run(action, [bulkItem(me, media)]);
          }}
        />
      ) : null}
      <ConfirmDialog
        confirm={confirm}
        onCancel={() => setConfirm(null)}
        onConfirm={({ kind, items: targets }) => {
          setConfirm(null);
          actions.run(kind, targets);
        }}
      />
    </>
  );
}
