// The media library (`/admin/media`, 07 §3): a page header (Media, the count of loaded
// items, **Upload**), a filter bar (search on `/`, kind chips, Mine, Trash, grid/list),
// the grid or the list with infinite paging by `next_cursor` (50 a page), bulk trash
// from the selection bar with Undo, the Trash view with Restore and Delete permanently,
// the detail sheet for `?id=` and the upload tray. The whole page takes dropped files;
// new uploads appear at the top with the layout animation. Actions show by capability
// (`rights.ts`); the server decides every call.

import {
  keepPreviousData,
  useInfiniteQuery,
  useQueryClient,
  type InfiniteData,
} from "@tanstack/react-query";
import { cn } from "cn";
import {
  ImageIcon,
  LayoutGridIcon,
  ListIcon,
  SearchXIcon,
  Trash2Icon,
  UploadIcon,
  UserRoundIcon,
} from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { qk } from "@/api/keys";
import { type MediaDto, listMedia } from "@/api/media";
import { ApiError } from "@/api/problem";
import type { SchemaMediaList } from "@/api/schema";
import type { MeDto } from "@/auth/session";
import type { RowSelectionState } from "@/components/kit/data-table-features";
import { DropZone } from "@/components/kit/drop-zone";
import { EmptyState } from "@/components/kit/empty-state";
import { ErrorState } from "@/components/kit/error-state";
import { ForbiddenState } from "@/components/kit/forbidden-state";
import { PageHeader } from "@/components/kit/page-header";
import { ScrollFade } from "@/components/kit/scroll-fade";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { BulkBar } from "@/content/bulk-bar";
import { trashUndoable } from "@/content/bulk";
import { Chip, SearchField } from "@/content/filters";
import { uiLocale } from "@/lib/locale";
import { useShortcut } from "@/lib/shortcuts";
import { displayTitle } from "./format";
import { type MediaAction, useMediaActions } from "./media-actions";
import { MediaDetail } from "./media-detail";
import { MediaGrid } from "./media-grid";
import {
  MEDIA_KINDS,
  type MediaSearch,
  type MediaView,
  effectiveView,
  hasFilters,
  mediaQuery,
  rememberView,
  searchDisables,
} from "./media-search";
import { MediaTable } from "./media-table";
import { type MediaBulkItem, bulkItem, canUpload } from "./rights";
import { UploadTray } from "./upload-tray";
import { useUploadQueue } from "./use-upload-queue";
import { useUploadRequest } from "./upload-intent";

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
  const forbidden = list.error instanceof ApiError && list.error.status === 403;

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
  const dim = list.isPlaceholderData ? "opacity-60 transition-opacity" : "transition-opacity";

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
      {forbidden ? (
        <ForbiddenState />
      ) : list.isError && !list.data ? (
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
          className={dim}
        />
      ) : (
        <MediaTable
          items={items}
          me={me}
          label={t("grid.label")}
          loading={list.isPending}
          empty={empty}
          footer={footer}
          selection={{ state: selectedIds, onChange: setSelected }}
          onOpen={open}
          className={dim}
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

function Toolbar({
  search,
  view,
  onChange,
  onView,
}: {
  search: MediaSearch;
  view: MediaView;
  onChange: MediaLibraryProps["setSearch"];
  onView: (view: MediaView) => void;
}) {
  const { t } = useTranslation("media");
  const trash = search.trash === true;
  const disabled = searchDisables(search);
  return (
    <div className="flex flex-col gap-3 lg:flex-row lg:items-center">
      {/* The trash has no search (trashed items leave the search index). */}
      {trash ? null : (
        <div role="search" className="w-full lg:w-auto lg:shrink-0">
          <SearchField
            label={t("searchLabel")}
            value={search.q ?? ""}
            onSearch={(q) => onChange({ q: q || undefined })}
          />
        </div>
      )}
      {/* Phones: the chips get a row of their own, Trash and the view toggle the next. */}
      <div className="flex min-w-0 flex-1 flex-col gap-3 sm:flex-row sm:items-center sm:gap-2">
        <ScrollFade className="-m-1 min-w-0 flex-1 p-1 md:overflow-visible">
          <div className="flex w-max items-center gap-2 md:w-auto md:flex-wrap">
            <div role="group" aria-label={t("kind.label")} className="flex items-center gap-1.5">
              <Chip
                pressed={search.kind === undefined}
                onClick={() => onChange({ kind: undefined })}
              >
                {t("kind.all")}
              </Chip>
              {MEDIA_KINDS.map((kind) => (
                <Chip
                  key={kind}
                  pressed={search.kind === kind}
                  onClick={() => onChange({ kind: search.kind === kind ? undefined : kind })}
                >
                  {t(`kind.${kind}`)}
                </Chip>
              ))}
            </div>
            <span aria-hidden="true" className="mx-0.5 h-5 w-px bg-border" />
            <Chip
              pressed={search.mine === true}
              disabledReason={disabled ? t("mineBySearch") : undefined}
              onClick={() => onChange({ mine: search.mine ? undefined : true })}
            >
              <UserRoundIcon data-icon="inline-start" />
              {t("mine")}
            </Chip>
            {hasFilters(search) ? (
              <Button
                variant="ghost"
                className="text-muted-foreground"
                onClick={() => onChange({ q: undefined, kind: undefined, mine: undefined })}
              >
                {t("clearFilters")}
              </Button>
            ) : null}
          </div>
        </ScrollFade>
        <div className="flex items-center justify-between gap-2">
          <Chip
            pressed={trash}
            onClick={() => onChange({ trash: trash ? undefined : true, id: undefined })}
          >
            <Trash2Icon data-icon="inline-start" />
            {t("trash.toggle")}
          </Chip>
          <ToggleGroup
            type="single"
            variant="outline"
            value={view}
            aria-label={t("view.label")}
            onValueChange={(v) => {
              if (v === "grid" || v === "list") onView(v);
            }}
          >
            <ToggleGroupItem
              value="grid"
              aria-label={t("view.grid")}
              className="pointer-coarse:size-11"
            >
              <LayoutGridIcon />
            </ToggleGroupItem>
            <ToggleGroupItem
              value="list"
              aria-label={t("view.list")}
              className="pointer-coarse:size-11"
            >
              <ListIcon />
            </ToggleGroupItem>
          </ToggleGroup>
        </div>
      </div>
    </div>
  );
}

function LibraryEmpty({
  search,
  uploads,
  onUpload,
  onClear,
}: {
  search: MediaSearch;
  uploads: boolean;
  onUpload: () => void;
  onClear: () => void;
}) {
  const { t } = useTranslation("media");
  const trash = search.trash === true;
  if (!trash && search.q !== undefined) {
    return (
      <EmptyState
        icon={SearchXIcon}
        title={t("empty.search.title", { q: search.q })}
        body={t("empty.search.body")}
        action={
          <Button variant="outline" onClick={onClear}>
            {t("empty.search.action")}
          </Button>
        }
      />
    );
  }
  if (hasFilters(search)) {
    return (
      <EmptyState
        icon={SearchXIcon}
        title={trash ? t("empty.trashFiltered.title") : t("empty.filtered.title")}
        body={t("empty.filtered.body")}
        action={
          <Button variant="outline" onClick={onClear}>
            {t("clearFilters")}
          </Button>
        }
      />
    );
  }
  if (trash) {
    return (
      <EmptyState icon={Trash2Icon} title={t("empty.trash.title")} body={t("empty.trash.body")} />
    );
  }
  return (
    <EmptyState
      icon={ImageIcon}
      title={t("empty.none.title")}
      body={uploads ? t("empty.none.body") : t("empty.none.bodyReadOnly")}
      action={
        uploads ? (
          <Button variant="outline" onClick={onUpload}>
            <UploadIcon data-icon="inline-start" />
            {t("empty.none.action")}
          </Button>
        ) : undefined
      }
    />
  );
}

/** Below the items: "Load more" plus a sentinel that loads the next page when seen. */
function LoadMore({
  hasNext,
  loadingNext,
  shown,
  onMore,
}: {
  hasNext: boolean;
  loadingNext: boolean;
  shown: number;
  onMore: () => void;
}) {
  const { t } = useTranslation("media");
  const sentinel = useRef<HTMLDivElement>(null);
  const latest = useRef(onMore);
  useEffect(() => {
    latest.current = onMore;
  });
  useEffect(() => {
    const el = sentinel.current;
    if (!el || !hasNext || typeof IntersectionObserver === "undefined") return;
    // Inside the list view's scrolling frame, else the page.
    const root = el.closest("[data-slot=data-table]");
    const io = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting)) latest.current();
      },
      { root, rootMargin: "0px 0px 240px 0px" },
    );
    io.observe(el);
    return () => io.disconnect();
  }, [hasNext, shown]);
  const number = new Intl.NumberFormat(uiLocale());
  return (
    <div className="flex flex-col items-center gap-2 px-4 py-3 [[data-slot=data-table]_&]:border-t">
      {hasNext ? (
        <>
          <div ref={sentinel} aria-hidden="true" className="h-px w-full" />
          <Button
            variant="ghost"
            onClick={onMore}
            disabled={loadingNext}
            aria-busy={loadingNext || undefined}
          >
            {loadingNext ? t("loadingMore") : t("loadMore")}
          </Button>
        </>
      ) : (
        <p className="text-sm text-muted-foreground tabular-nums">
          {t("end", { count: shown, formatted: number.format(shown) })}
        </p>
      )}
    </div>
  );
}

interface Confirm {
  kind: "delete" | "trash";
  items: MediaBulkItem[];
}

/** Delete permanently, or a trash nothing could undo, asks first (07 §3). */
function ConfirmDialog({
  confirm,
  onCancel,
  onConfirm,
}: {
  confirm: Confirm | null;
  onCancel: () => void;
  onConfirm: (confirm: Confirm) => void;
}) {
  const { t } = useTranslation("media");
  const count = confirm?.items.length ?? 0;
  const first = confirm?.items[0];
  const kind = confirm?.kind ?? "delete";
  const title = first ? displayTitle(first.media) : "";
  return (
    <AlertDialog
      open={confirm !== null && count > 0}
      onOpenChange={(o) => {
        if (!o) onCancel();
      }}
    >
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{t(`confirm.${kind}.title`, { count })}</AlertDialogTitle>
          <AlertDialogDescription>
            {count === 1
              ? t(`confirm.${kind}.bodyOne`, { title })
              : t(`confirm.${kind}.bodyMany`, { count })}
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>{t("confirm.cancel")}</AlertDialogCancel>
          <Button
            variant="destructive"
            className={cn(
              "bg-destructive text-background hover:bg-destructive/90 dark:bg-destructive dark:hover:bg-destructive/90",
            )}
            onClick={() => {
              if (confirm) onConfirm(confirm);
            }}
          >
            {t(`confirm.${kind}.confirm`, { count })}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
