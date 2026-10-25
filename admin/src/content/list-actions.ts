// What the list does to items, one or many (M2b P20, P21): trash with an Undo toast,
// restore, publish and delete permanently, each a `runBulk` fan-out. Failures stay on
// screen as a report (a toast would vanish); a `401` or a forced password change among
// them is re-thrown, so the global handler sends the user to sign in as for any write.
// After every run the type's lists and counts are refetched.

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
  type Trashed,
  type UndoNotice,
  type UndoTransitions,
  runBulk,
  undoTrash,
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

type Run =
  | { action: Exclude<BulkAction, "trash">; items: ContentSummary[] }
  | { action: "trash"; items: ContentSummary[] }
  | { action: "undo"; entries: Trashed[] };

/** Problems the global handler must see (sign in again, change password). */
const REDIRECTING = new Set(["unauthenticated", "password-reset-required"]);

export function useListActions(type: string, options: { onDone?: (ok: string[]) => void } = {}) {
  const { t } = useTranslation("content");
  const qc = useQueryClient();
  const [report, setReport] = useState<FailureReport | null>(null);

  const ops: UndoTransitions = {
    restore: (id, v) => lifecycle(type, id, "restore", v),
    publish: (id, v) => lifecycle(type, id, "publish", v),
    submit: (id, v) => lifecycle(type, id, "submit", v),
    schedule: (id, v, at) => lifecycle(type, id, "schedule", v, { at }),
  };

  const refresh = () =>
    Promise.all([
      qc.invalidateQueries({ queryKey: qk.content.lists(type) }),
      qc.invalidateQueries({ queryKey: qk.content.counts(type) }),
    ]);

  const mutation = useMutation({
    mutationFn: async (run: Run) => {
      const titles = new Map<string, string>();
      let result: BulkResult;
      let trashed: Trashed[] = [];
      const notices: UndoNotice[] = [];
      let total: number;
      if (run.action === "undo") {
        for (const e of run.entries) titles.set(e.after.id, e.after.title);
        const byId = new Map(run.entries.map((e) => [e.after.id, e]));
        const items = run.entries.map((e) => e.after);
        total = items.length;
        result = await runBulk(items, "restore", async (item: SchemaContentDto) => {
          const entry = byId.get(item.id);
          if (!entry) return;
          const undone = await undoTrash(entry, ops);
          if (undone.notice) notices.push(undone.notice);
        });
      } else {
        for (const item of run.items) titles.set(item.id, item.title);
        total = run.items.filter((i) => i.allowed[run.action]).length;
        const done: Trashed[] = [];
        result = await runBulk(run.items, run.action, async (item) => {
          switch (run.action) {
            case "trash": {
              const after = await lifecycle(type, item.id, "trash", item.version);
              done.push({ before: item, after });
              return;
            }
            case "restore":
              await lifecycle(type, item.id, "restore", item.version);
              return;
            case "publish":
              await lifecycle(type, item.id, "publish", item.version);
              return;
            case "delete":
              await deleteContent(type, item.id, item.version);
              return;
          }
        });
        // In the order of the list, not of completion.
        const order = new Map(run.items.map((item, i) => [item.id, i]));
        trashed = done.sort((a, b) => (order.get(a.after.id) ?? 0) - (order.get(b.after.id) ?? 0));
      }
      const redirect = result.failed.find((f) => REDIRECTING.has(f.error.slug));
      if (redirect) throw redirect.error;
      return { run, result, trashed, notices, titles, total };
    },
    onSuccess: ({ run, result, trashed, notices, titles, total }) => {
      setReport(
        result.failed.length > 0
          ? {
              action: run.action,
              total,
              failed: result.failed.map((f) => ({
                ...f,
                title: titles.get(f.id) || t("row.untitled"),
              })),
            }
          : null,
      );
      const count = result.ok.length;
      if (count > 0) {
        if (run.action === "trash") {
          toast.success(t("toast.trashed", { count }), {
            duration: UNDO_TOAST_MS,
            action: {
              label: t("toast.undo"),
              onClick: () => {
                mutation.mutate({ action: "undo", entries: trashed });
              },
            },
          });
        } else if (run.action === "undo") {
          const kinds = [...new Set(notices)];
          toast.success(t("toast.undone", { count }), {
            ...(kinds.length > 0
              ? {
                  description: kinds
                    .map((k) =>
                      t(`toast.notice.${k}`, { count: notices.filter((n) => n === k).length }),
                    )
                    .join(" "),
                }
              : {}),
          });
        } else {
          toast.success(t(`toast.${run.action}`, { count }));
        }
      }
      options.onDone?.(result.ok);
    },
    onSettled: () => refresh(),
  });

  return {
    run: (action: BulkAction, items: ContentSummary[]) => {
      mutation.mutate({ action, items });
    },
    pending: mutation.isPending ? (mutation.variables?.action ?? null) : null,
    report,
    dismissReport: () => setReport(null),
  };
}
