// What the library does to media items, one or many: trash with an Undo toast, restore
// and delete permanently (from the Trash view), each a `runBulk` fan-out (4 at a time,
// each with the item's own `If-Match`, only for items `media/rights.ts` allows). Undo
// restores every item the run trashed and the viewer may restore; a trash answers `204`
// without the new version, so Undo reads each item first and restores with its version.
// Failures (a `403`, a `412` from an edit made elsewhere) are shown as an error toast
// with the first problem's detail; the lists refetch after every run.

import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { qk } from "@/api/keys";
import { deleteMedia, getMedia, restoreMedia, trashMedia } from "@/api/media";
import { type BulkResult, REDIRECTING, runBulk } from "@/content/bulk";
import { UNDO_TOAST_MS } from "@/content/list-actions";
import type { MediaBulkItem } from "./rights";

export type MediaAction = "trash" | "restore" | "delete";
type Run = { action: MediaAction | "undo"; items: MediaBulkItem[] };

function request(action: MediaAction | "undo", item: MediaBulkItem): Promise<unknown> {
  const { id, version } = item.media;
  switch (action) {
    case "trash":
      return trashMedia(id, version);
    case "restore":
      return restoreMedia(id, version);
    case "delete":
      return deleteMedia(id, version);
    case "undo":
      return getMedia(id).then((current) => restoreMedia(id, current.version));
  }
}

export function useMediaActions(options: { onDone?: (ok: string[]) => void } = {}) {
  const { t } = useTranslation("media");
  const qc = useQueryClient();

  const mutation = useMutation({
    mutationFn: async (run: Run): Promise<{ run: Run; result: BulkResult }> => {
      let result: BulkResult;
      try {
        result = await runBulk(run.items, run.action === "undo" ? "restore" : run.action, (item) =>
          request(run.action, item),
        );
      } finally {
        await qc.invalidateQueries({ queryKey: qk.media.all });
      }
      const redirect = result.failed.find((f) => REDIRECTING.has(f.error.slug));
      if (redirect) throw redirect.error;
      return { run, result };
    },
    onSuccess: ({ run, result }) => {
      const count = result.ok.length;
      if (result.failed.length > 0) {
        const first = result.failed[0]?.error;
        const detail =
          first && first.status !== 0
            ? first.problem.detail || first.problem.title
            : t("toast.network");
        toast.error(t(`toast.failed.${run.action}`, { count: result.failed.length }), {
          description: detail,
        });
      }
      if (count > 0) {
        if (run.action === "trash") trashToast(run.items.filter((i) => result.ok.includes(i.id)));
        else if (run.action === "undo") toast.success(t("toast.undone", { count }));
        else toast.success(t(`toast.${run.action}`, { count }));
      }
      options.onDone?.(result.ok);
    },
  });

  /** "N moved to trash", with Undo for the ones this viewer may restore. */
  const trashToast = (trashed: MediaBulkItem[]) => {
    const count = trashed.length;
    const undoable = trashed
      .filter((i) => i.allowed.trash_undoable)
      .map((i) => ({ ...i, allowed: { ...i.allowed, restore: true } }));
    if (undoable.length === 0) {
      toast.success(t("toast.trash", { count }));
      return;
    }
    toast.success(t("toast.trash", { count }), {
      duration: UNDO_TOAST_MS,
      action: {
        label: t("toast.undoLabel"),
        onClick: () => {
          mutation.mutate({ action: "undo", items: undoable });
        },
      },
    });
  };

  return {
    run: (action: MediaAction, items: MediaBulkItem[]) => {
      mutation.mutate({ action, items });
    },
    pending: mutation.isPending ? (mutation.variables?.action ?? null) : null,
  };
}
