// The comments queue's other pieces (`queue.tsx`): the status tabs, the empty state,
// the report of a run's failures and the delete-permanently confirm.

import { Link } from "@tanstack/react-router";
import { CircleAlertIcon, InboxIcon, ShieldAlertIcon, Trash2Icon, XIcon } from "lucide-react";
import { useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import type { CommentDto } from "@/api/comments";
import type { ApiError } from "@/api/problem";
import type { SchemaCommentCountsDto } from "@/api/schema";
import { EmptyState } from "@/components/kit/empty-state";
import { ScrollFade } from "@/components/kit/scroll-fade";
import { TAB_LIST, tabCountClass, tabLinkClass } from "@/components/kit/tab-styles";
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
import { DESTRUCTIVE } from "@/lib/destructive";
import { uiLocale } from "@/lib/locale";
import {
  COMMENT_STATUSES,
  type CommentSearch,
  type CommentStatus,
  parseCommentSearch,
  statusOfSearch,
  withStatus,
} from "./comment-search";

/** What a run of the last action failed on, listed until dismissed. */
export interface Report {
  total: number;
  failed: { id: string; name: string; error: ApiError }[];
}

/** Pending, Approved, Spam, Trash: links (each a URL), with their counts. */
export function StatusTabs({
  search,
  counts,
}: {
  search: CommentSearch;
  counts: SchemaCommentCountsDto | undefined;
}) {
  const { t } = useTranslation("comments");
  const current = statusOfSearch(search);
  const number = new Intl.NumberFormat(uiLocale());
  return (
    <nav aria-label={t("tabs.label")}>
      <ScrollFade activeSelector={`[data-tab="${current}"]`} activeKey={current}>
        <ul className={TAB_LIST}>
          {COMMENT_STATUSES.map((tab) => {
            const active = tab === current;
            const count = counts?.[tab];
            return (
              <li key={tab}>
                <Link
                  to="/comments"
                  search={(prev: Record<string, unknown>) =>
                    withStatus(parseCommentSearch(prev), tab)
                  }
                  activeOptions={{ exact: true, includeSearch: true }}
                  aria-current={active ? "page" : undefined}
                  data-tab={tab}
                  className={tabLinkClass(active)}
                >
                  {t(`tabs.${tab}`)}
                  {count !== undefined ? (
                    <span data-slot="tab-count" className={tabCountClass(active)}>
                      {number.format(count)}
                    </span>
                  ) : null}
                </Link>
              </li>
            );
          })}
        </ul>
      </ScrollFade>
    </nav>
  );
}

export function QueueEmpty({
  status,
  filtered,
  onClear,
}: {
  status: CommentStatus;
  filtered: boolean;
  onClear: () => void;
}) {
  const { t } = useTranslation("comments");
  const icon = status === "trash" ? Trash2Icon : status === "spam" ? ShieldAlertIcon : InboxIcon;
  return (
    <EmptyState
      icon={icon}
      title={t(`empty.${status}.title`)}
      body={filtered ? t("empty.filtered.body") : t(`empty.${status}.body`)}
      action={
        filtered ? (
          <Button variant="outline" onClick={onClear}>
            {t("empty.filtered.action")}
          </Button>
        ) : undefined
      }
      className="border-0"
    />
  );
}

/** The failures of the last run (a toast would vanish). */
export function FailurePanel({ report, onDismiss }: { report: Report; onDismiss: () => void }) {
  const { t } = useTranslation("comments");
  const reason = (error: ApiError): ReactNode => {
    if (error.status === 0) return t("report.network");
    if (error.slug === "not-found") return t("report.gone");
    return error.problem.detail || error.problem.title;
  };
  return (
    <section
      role="alert"
      aria-labelledby="comment-failures"
      data-slot="failure-report"
      className="flex gap-3 rounded-xl border border-destructive/40 bg-destructive/5 p-4"
    >
      <CircleAlertIcon className="mt-0.5 size-5 shrink-0 text-destructive" aria-hidden="true" />
      <div className="flex min-w-0 flex-1 flex-col gap-2">
        <h2 id="comment-failures" className="font-medium text-foreground">
          {t("report.title", { count: report.failed.length, total: report.total })}
        </h2>
        <ul className="flex flex-col gap-1.5 text-sm">
          {report.failed.map((f) => (
            <li key={f.id} className="flex min-w-0 flex-wrap gap-x-2">
              <span className="max-w-full truncate font-medium" title={f.name}>
                {f.name}
              </span>
              <span className="text-muted-foreground">{reason(f.error)}</span>
            </li>
          ))}
        </ul>
      </div>
      <Button variant="ghost" size="icon-sm" aria-label={t("report.dismiss")} onClick={onDismiss}>
        <XIcon />
      </Button>
    </section>
  );
}

/** Delete permanently: asks first, and says the replies go too (the server's rule). */
export function DeleteDialog({
  targets,
  onCancel,
  onConfirm,
}: {
  targets: CommentDto[] | null;
  onCancel: () => void;
  onConfirm: (targets: CommentDto[]) => void;
}) {
  const { t } = useTranslation("comments");
  // The last targets, so the dialog keeps its text while it closes.
  const [shown, setShown] = useState<CommentDto[]>([]);
  if (targets !== null && targets !== shown) setShown(targets);
  const list = targets ?? shown;
  const count = list.length;
  return (
    <AlertDialog
      open={targets !== null && targets.length > 0}
      onOpenChange={(open) => {
        if (!open) onCancel();
      }}
    >
      <AlertDialogContent data-slot="delete-dialog">
        <AlertDialogHeader>
          <AlertDialogTitle>{t("delete.title", { count })}</AlertDialogTitle>
          <AlertDialogDescription className="break-words">
            {t("delete.body", { count, name: list[0]?.author_name ?? "" })}
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>{t("delete.cancel")}</AlertDialogCancel>
          <Button
            variant="destructive"
            className={DESTRUCTIVE}
            onClick={() => {
              if (targets) onConfirm(targets);
            }}
          >
            {t("delete.confirm", { count })}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
