// The media library's other pieces (`library.tsx`): its empty states, the "Load more"
// footer with its sentinel, and the confirm for a delete or an unrecoverable trash.

import { cn } from "cn";
import { ImageIcon, SearchXIcon, Trash2Icon, UploadIcon } from "lucide-react";
import { useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";
import { EmptyState } from "@/components/kit/empty-state";
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
import { uiLocale } from "@/lib/locale";
import { displayTitle } from "./format";
import { type MediaSearch, hasFilters } from "./media-search";
import type { MediaBulkItem } from "./rights";

export function LibraryEmpty({
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
export function LoadMore({
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

export interface Confirm {
  kind: "delete" | "trash";
  items: MediaBulkItem[];
}

/** Delete permanently, or a trash nothing could undo, asks first (07 §3). */
export function ConfirmDialog({
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
