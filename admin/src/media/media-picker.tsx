// The media picker (07 §3 Editor; 11 §7): a dialog over any screen that hands back one
// media item. Two tabs: Library (a search and the library's grid with single selection,
// 50 a page with "Load more") and Upload (a drop area with a file chooser, the same queue
// as the library: at most 3 at once, one `Idempotency-Key` per file). A finished upload
// joins the top of the library, selected, and can be chosen at once, before its variants
// exist. `enter` on a tile (or a double click) chooses it, Select chooses the selected
// one, `esc` cancels. Opened for the featured image or an inline image it shows images
// only; for a `media` field every kind. Upload shows only with `upload_media` (the server
// decides again).
//
// `openMediaPicker` works from anywhere (an editor command, a field); `MediaPickerHost`,
// mounted once in the shell, shows the dialog. Closing it while its uploads run asks
// first (Keep uploading / Cancel uploads): an upload is never cancelled silently. The promise settles once the dialog has
// closed and returned focus, so the caller can move focus on (into the editor).

import { keepPreviousData, useInfiniteQuery } from "@tanstack/react-query";
import { ImageIcon, LibraryIcon, SearchXIcon, UploadIcon } from "lucide-react";
import { type RefObject, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { qk } from "@/api/keys";
import { type MediaDto, listMedia } from "@/api/media";
import { useMe } from "@/auth/session";
import type { RowSelectionState } from "@/components/kit/data-table-features";
import { DropZone } from "@/components/kit/drop-zone";
import { EmptyState } from "@/components/kit/empty-state";
import { ErrorState } from "@/components/kit/error-state";
import { MediaThumb } from "@/components/kit/media-thumb";
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
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { SearchField } from "@/content/filters";
import { displayTitle, typeLabel } from "./format";
import { MediaGrid } from "./media-grid";
import {
  type PickerOptions,
  type PickerRequest,
  registerPickerHost,
  requestMediaPicker,
  settlePicker,
  usePickerRequest,
} from "./picker-state";
import { canUpload } from "./rights";
import { UploadRow } from "./upload-tray";
import { useUploadQueue } from "./use-upload-queue";

/** Opens the picker; resolves to the chosen item, or `null` when cancelled. */
export function openMediaPicker(opts: PickerOptions): Promise<MediaDto | null> {
  return requestMediaPicker(opts);
}

/** The picker's dialog, for every `openMediaPicker` (mounted once, in the shell). */
export function MediaPickerHost() {
  const request = usePickerRequest();
  useEffect(() => registerPickerHost(), []);
  // The request whose dialog is closing (its choice settles once focus has returned).
  const [closing, setClosing] = useState<number | null>(null);
  const chosen = useRef<MediaDto | null>(null);
  const open = request !== null && closing !== request.id;
  // The body's guarded close: it asks first while uploads run (set by the body).
  const guarded = useRef<((media: MediaDto | null) => void) | null>(null);
  const close = (media: MediaDto | null) => {
    if (!request) return;
    chosen.current = media;
    setClosing(request.id);
  };
  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!next) (guarded.current ?? close)(null);
      }}
    >
      {request ? (
        <DialogContent
          data-slot="media-picker"
          className="flex h-[min(46rem,calc(100dvh-2rem))] flex-col gap-0 overflow-hidden p-0 sm:max-w-4xl"
          onCloseAutoFocus={() => {
            const id = request.id;
            const media = chosen.current;
            chosen.current = null;
            // After Radix has put focus back, so the caller can move it on.
            window.setTimeout(() => settlePicker(id, media), 0);
          }}
        >
          <PickerBody key={request.id} request={request} onChoose={close} guardRef={guarded} />
        </DialogContent>
      ) : null}
    </Dialog>
  );
}

type Tab = "library" | "upload";

function PickerBody({
  request,
  onChoose: choose,
  guardRef,
}: {
  request: PickerRequest;
  onChoose: (media: MediaDto | null) => void;
  /** Receives the close that asks first while uploads run. */
  guardRef: RefObject<((media: MediaDto | null) => void) | null>;
}) {
  const { t } = useTranslation("media");
  const me = useMe();
  const uploads = me ? canUpload(me) : false;
  const images = request.kind === "image";
  const [tab, setTab] = useState<Tab>("library");
  const [q, setQ] = useState("");
  const filters = useMemo(
    () => ({ ...(request.kind ? { kind: request.kind } : {}), ...(q ? { q } : {}) }),
    [request.kind, q],
  );
  const list = useInfiniteQuery({
    queryKey: qk.media.infinite(filters),
    queryFn: ({ pageParam, signal }) =>
      listMedia(pageParam ? { ...filters, cursor: pageParam } : filters, { signal }),
    initialPageParam: "",
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    placeholderData: keepPreviousData,
  });
  // Uploads made here, newest first: shown at the top of the library at once.
  const [uploaded, setUploaded] = useState<MediaDto[]>([]);
  const items = useMemo(() => {
    const seen = new Map<string, MediaDto>();
    const fresh = q ? [] : uploaded.filter((m) => fits(request.kind, m));
    for (const m of fresh) seen.set(m.id, m);
    for (const page of list.data?.pages ?? []) {
      for (const m of page.items) if (!seen.has(m.id)) seen.set(m.id, m);
    }
    return [...seen.values()];
  }, [list.data, uploaded, q, request.kind]);
  const [selection, setSelection] = useState<RowSelectionState>({});
  const selectedId = Object.keys(selection).find((id) => selection[id]);
  const selected =
    items.find((m) => m.id === selectedId) ?? uploaded.find((m) => m.id === selectedId) ?? null;

  const { queue, entries } = useUploadQueue((media) => {
    setUploaded((u) => [media, ...u.filter((m) => m.id !== media.id)]);
    setSelection({ [media.id]: true });
  });
  const active = entries.some((e) => e.state === "queued" || e.state === "uploading");
  const done = entries.some((e) => e.state === "done");
  const failed = entries.some((e) => e.state === "failed");
  // Every upload landed: back to the library, the newest one selected.
  const wasActive = useRef(false);
  useEffect(() => {
    if (wasActive.current && !active && done && !failed) setTab("library");
    wasActive.current = active;
  }, [active, done, failed]);

  const what = images ? t("picker.images") : t("picker.files");

  // Closing cancels the uploads still running: never silently (Keep uploading / Cancel
  // uploads), also when an item is chosen meanwhile.
  const [asking, setAsking] = useState<{ media: MediaDto | null } | null>(null);
  const onChoose = (media: MediaDto | null) => {
    if (active) setAsking({ media });
    else choose(media);
  };
  useEffect(() => {
    guardRef.current = onChoose;
    return () => {
      guardRef.current = null;
    };
  });

  return (
    <Tabs
      value={tab}
      onValueChange={(v) => setTab(v as Tab)}
      className="flex min-h-0 flex-1 flex-col gap-0"
    >
      <DialogHeader className="gap-3 border-b px-5 pt-5 pb-3 sm:px-6">
        <div className="flex flex-col gap-1 pr-8">
          <DialogTitle>{request.title}</DialogTitle>
          <DialogDescription>
            {uploads
              ? t("picker.description", { what })
              : t("picker.descriptionNoUpload", { what })}
          </DialogDescription>
        </div>
        <div className="flex flex-wrap items-center gap-3">
          {uploads ? (
            <TabsList>
              <TabsTrigger value="library" className="pointer-coarse:min-h-11">
                <LibraryIcon data-icon="inline-start" />
                {t("picker.library")}
              </TabsTrigger>
              <TabsTrigger value="upload" className="pointer-coarse:min-h-11">
                <UploadIcon data-icon="inline-start" />
                {t("picker.upload")}
              </TabsTrigger>
            </TabsList>
          ) : null}
          {tab === "library" ? (
            <div className="min-w-0 basis-full sm:basis-auto">
              <SearchField label={what} value={q} onSearch={setQ} />
            </div>
          ) : null}
        </div>
      </DialogHeader>

      <TabsContent
        value="library"
        className="min-h-0 flex-1 overflow-y-auto overscroll-contain px-5 py-4 sm:px-6"
      >
        {list.isError ? (
          <ErrorState error={list.error} retry={() => void list.refetch()} />
        ) : (
          <MediaGrid
            single
            items={items}
            label={t("picker.gridLabel", { what })}
            loading={list.isPending && uploaded.length === 0}
            selection={{ state: selection, onChange: setSelection }}
            onOpen={(m) => onChoose(m)}
            empty={
              q ? (
                <EmptyState
                  icon={SearchXIcon}
                  title={t("picker.noMatchTitle")}
                  body={t("picker.noMatchBody", { q })}
                />
              ) : (
                <EmptyState
                  icon={ImageIcon}
                  title={t("picker.emptyTitle", { what })}
                  body={uploads ? t("picker.emptyBody") : t("picker.emptyBodyNoUpload")}
                  {...(uploads
                    ? {
                        action: (
                          <Button variant="outline" onClick={() => setTab("upload")}>
                            <UploadIcon data-icon="inline-start" />
                            {t("picker.upload")}
                          </Button>
                        ),
                      }
                    : {})}
                />
              )
            }
            footer={
              list.hasNextPage ? (
                <div className="flex justify-center">
                  <Button
                    variant="outline"
                    disabled={list.isFetchingNextPage}
                    onClick={() => void list.fetchNextPage()}
                  >
                    {list.isFetchingNextPage ? t("picker.loadingMore") : t("picker.loadMore")}
                  </Button>
                </div>
              ) : null
            }
          />
        )}
      </TabsContent>

      {uploads ? (
        <TabsContent
          value="upload"
          className="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto overscroll-contain px-5 py-4 sm:px-6"
        >
          <DropZone
            onFiles={(files) => queue.add(files)}
            {...(images ? { accept: "image/*" } : {})}
            className="min-h-48 rounded-xl border border-dashed p-5"
          >
            <p className="text-sm text-muted-foreground">
              {images ? t("picker.dropImages") : t("picker.dropFiles")}
            </p>
          </DropZone>
          {entries.length > 0 ? (
            <ul aria-label={t("picker.uploads")} className="rounded-xl border">
              {entries.map((entry) => (
                <UploadRow
                  key={entry.id}
                  entry={entry}
                  queue={queue}
                  onOpen={(m) => onChoose(m)}
                  openLabel={{
                    text: t("picker.select"),
                    name: (name) => t("picker.selectName", { name }),
                  }}
                />
              ))}
            </ul>
          ) : null}
        </TabsContent>
      ) : null}

      <div className="flex flex-wrap items-center gap-x-4 gap-y-3 border-t bg-muted/40 px-5 py-3 sm:px-6">
        <div className="flex min-w-0 flex-1 basis-56 items-center gap-3" aria-live="polite">
          {selected ? (
            <>
              <MediaThumb media={selected} size="sm" caption={false} />
              <div className="flex min-w-0 flex-col">
                <span
                  className="truncate text-sm font-medium text-foreground"
                  title={displayTitle(selected)}
                >
                  {displayTitle(selected)}
                </span>
                <span className="truncate text-xs text-muted-foreground">
                  {typeLabel(selected)}
                </span>
              </div>
            </>
          ) : (
            <span className="text-sm text-muted-foreground">{t("picker.noneSelected")}</span>
          )}
        </div>
        <div className="ml-auto flex gap-2">
          <Button variant="outline" onClick={() => onChoose(null)}>
            {t("picker.cancel")}
          </Button>
          <Button disabled={selected === null} onClick={() => onChoose(selected)}>
            {t("picker.select")}
          </Button>
        </div>
      </div>
      <AlertDialog
        open={asking !== null}
        onOpenChange={(open) => {
          if (!open) setAsking(null);
        }}
      >
        <AlertDialogContent data-slot="picker-uploads-dialog" className="sm:max-w-md">
          <AlertDialogHeader>
            <AlertDialogTitle>{t("picker.uploadsTitle")}</AlertDialogTitle>
            <AlertDialogDescription>{t("picker.uploadsBody")}</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel autoFocus onClick={() => setAsking(null)}>
              {t("picker.keepUploading")}
            </AlertDialogCancel>
            <Button
              variant="destructive"
              onClick={() => {
                const pending = asking;
                setAsking(null);
                queue.dispose();
                choose(pending?.media ?? null);
              }}
            >
              {t("picker.cancelUploads")}
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </Tabs>
  );
}

/** Whether an upload belongs in a picker for `kind` (by its sniffed MIME family). */
function fits(kind: PickerOptions["kind"], media: MediaDto): boolean {
  if (!kind) return true;
  const family = media.mime.split("/")[0] ?? "";
  return kind === "document" ? !["image", "video", "audio"].includes(family) : family === kind;
}
