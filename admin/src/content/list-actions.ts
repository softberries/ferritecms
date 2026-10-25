// What the list does to items, one or many (M2b P20, P21): trash with an Undo toast,
// restore, publish and delete permanently, each a `runBulk` fan-out. Every request uses
// the item's own type (never the open route's: the toast outlives a navigation), and
// the affected types' lists and counts are refetched after every run. Failures stay on
// screen as a report (a toast would vanish), for the list they happened on; a `401` or
// a forced password change among them is re-thrown, so the global handler sends the
// user to sign in as for any write. Undo is offered only for trashed items the actor
// may restore (`allowed.restore` of the trashed item); the list asks before a trash
// that nothing could undo (`trashUndoable`).

import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { deleteContent, lifecycle } from "@/api/content";
import { qk } from "@/api/keys";
import type { ApiError } from "@/api/problem";
import type { SchemaContentDto } from "@/api/schema";
import {
  type BulkAction,
  type BulkResult,
  REDIRECTING,
  type Trashed,
  type UndoNotice,
  type UndoTransitions,
  runBulk,
  undoTrash,
  undoableEntries,
} from "./bulk";
import type { ContentSummary } from "./list-columns";

/** How long the Undo toast stays (long enough to notice a slip). */
export const UNDO_TOAST_MS = 10_000;

export interface FailureReport {
  action: BulkAction | "undo";
  /** How many items the run was for. */
  total: number;
  failed: { id: string; title: string; error: ApiError }[];
}

type Run = { action: BulkAction; items: ContentSummary[] } | { action: "undo"; entries: Trashed[] };

/** The lifecycle calls of an undo, on the item's own type. */
function transitionsFor(type: string): UndoTransitions {
  return {
    restore: (id, v) => lifecycle(type, id, "restore", v),
    publish: (id, v) => lifecycle(type, id, "publish", v),
    submit: (id, v) => lifecycle(type, id, "submit", v),
    schedule: (id, v, at) => lifecycle(type, id, "schedule", v, { at }),
  };
}

/** One request of a bulk action on `item` (its own type and version). */
function request(action: BulkAction, item: ContentSummary): Promise<SchemaContentDto | void> {
  switch (action) {
    case "trash":
      return lifecycle(item.type, item.id, "trash", item.version);
    case "restore":
      return lifecycle(item.type, item.id, "restore", item.version);
    case "publish":
      return lifecycle(item.type, item.id, "publish", item.version);
    case "delete":
      return deleteContent(item.type, item.id, item.version);
  }
}

export function useListActions(type: string, options: { onDone?: (ok: string[]) => void } = {}) {
  const { t } = useTranslation("content");
  const qc = useQueryClient();
  // The report belongs to the list it happened on (the route is reused across types).
  const [report, setReport] = useState<{ type: string; report: FailureReport } | null>(null);

  const refresh = (types: Iterable<string>) =>
    Promise.all(
      [...new Set(types)].flatMap((ty) => [
        qc.invalidateQueries({ queryKey: qk.content.lists(ty) }),
        qc.invalidateQueries({ queryKey: qk.content.counts(ty) }),
      ]),
    );

  const mutation = useMutation({
    mutationFn: async (run: Run) => {
      const titles = new Map<string, string>();
      const types = new Set<string>();
      const notices: UndoNotice[] = [];
      let trashed: Trashed[] = [];
      let result: BulkResult;
      let total: number;
      try {
        if (run.action === "undo") {
          for (const e of run.entries) {
            titles.set(e.after.id, e.after.title);
            types.add(e.after.type);
          }
          const byId = new Map(run.entries.map((e) => [e.after.id, e]));
          const items = run.entries.map((e) => e.after);
          total = items.filter((i) => i.allowed.restore).length;
          result = await runBulk(items, "restore", async (item: SchemaContentDto) => {
            const entry = byId.get(item.id);
            if (!entry) return;
            const undone = await undoTrash(entry, transitionsFor(item.type));
            if (undone.notice) notices.push(undone.notice);
          });
        } else {
          const { action, items } = run;
          for (const item of items) {
            titles.set(item.id, item.title);
            types.add(item.type);
          }
          total = items.filter((i) => i.allowed[action]).length;
          const done: Trashed[] = [];
          result = await runBulk(items, action, async (item) => {
            const after = await request(action, item);
            if (action === "trash" && after) done.push({ before: item, after });
          });
          // In the order of the list, not of completion.
          const order = new Map(items.map((item, i) => [item.id, i]));
          trashed = done.sort(
            (a, b) => (order.get(a.after.id) ?? 0) - (order.get(b.after.id) ?? 0),
          );
        }
      } finally {
        await refresh(types);
      }
      const redirect = result.failed.find((f) => REDIRECTING.has(f.error.slug));
      if (redirect) throw redirect.error;
      return { run, result, trashed, notices, titles, total };
    },
    onSuccess: ({ run, result, trashed, notices, titles, total }) => {
      setReport(
        result.failed.length > 0
          ? {
              type,
              report: {
                action: run.action,
                total,
                failed: result.failed.map((f) => ({
                  ...f,
                  title: titles.get(f.id) || t("row.untitled"),
                })),
              },
            }
          : null,
      );
      const count = result.ok.length;
      if (count > 0) {
        if (run.action === "trash") trashToast(count, trashed);
        else if (run.action === "undo") undoneToast(count, notices);
        else toast.success(t(`toast.${run.action}`, { count }));
      }
      options.onDone?.(result.ok);
    },
  });

  /** "N moved to trash", with Undo for the ones this actor may restore. */
  const trashToast = (count: number, trashed: Trashed[]) => {
    const undoable = undoableEntries(trashed);
    if (undoable.length === 0) {
      toast.success(t("toast.trashed", { count }), {
        description: t("toast.trashedNoUndo", { count }),
      });
      return;
    }
    toast.success(t("toast.trashed", { count }), {
      duration: UNDO_TOAST_MS,
      ...(undoable.length < count
        ? { description: t("toast.trashedSomeUndo", { count: undoable.length, total: count }) }
        : {}),
      action: {
        label: t("toast.undo"),
        onClick: () => {
          mutation.mutate({ action: "undo", entries: undoable });
        },
      },
    });
  };

  const undoneToast = (count: number, notices: UndoNotice[]) => {
    const kinds = [...new Set(notices)];
    toast.success(t("toast.undone", { count }), {
      ...(kinds.length > 0
        ? {
            description: kinds
              .map((k) => t(`toast.notice.${k}`, { count: notices.filter((n) => n === k).length }))
              .join(" "),
          }
        : {}),
    });
  };

  return {
    run: (action: BulkAction, items: ContentSummary[]) => {
      mutation.mutate({ action, items });
    },
    pending: mutation.isPending ? (mutation.variables?.action ?? null) : null,
    report: report?.type === type ? report.report : null,
    dismissReport: () => setReport(null),
  };
}
