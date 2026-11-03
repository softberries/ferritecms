// A release's entries (design 15 §11.3): the item (opening its editor, or the staged
// change in change mode), its type and language, chips for what goes live ("Changes",
// "Publish", "Unpublish"), its review state, and a conflict badge with Rebase when the
// item was saved since the entry's base. Remove takes an entry out while the release is
// editable.

import { Link } from "@tanstack/react-router";
import { GitMergeIcon, TriangleAlertIcon, XIcon } from "lucide-react";
import { cn } from "cn";
import { useTranslation } from "react-i18next";
import { type ReleaseEntry, entryActions } from "@/api/releases";
import { Button } from "@/components/ui/button";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";

export interface EntryTableProps {
  releaseId: string;
  entries: ReleaseEntry[];
  /** Entries may change (open or failed). */
  editable: boolean;
  busy: boolean;
  onRebase: (entry: ReleaseEntry) => void;
  onRemove: (entry: ReleaseEntry) => void;
}

export function Chip({ kind }: { kind: "changes" | "publish" | "unpublish" }) {
  const { t } = useTranslation("releases");
  return (
    <span
      data-slot="entry-chip"
      data-kind={kind}
      className={cn(
        "inline-flex h-6 items-center rounded-md border px-2 text-xs font-medium",
        kind === "changes" && "border-primary/30 bg-primary/6 text-foreground",
        kind === "publish" && "text-status-published",
        kind === "unpublish" && "text-status-trash",
      )}
    >
      {t(`chips.${kind}`)}
    </span>
  );
}

export function EntryTable({
  releaseId,
  entries,
  editable,
  busy,
  onRebase,
  onRemove,
}: EntryTableProps) {
  const { t } = useTranslation("releases");
  if (entries.length === 0) {
    return (
      <p
        data-slot="entries-empty"
        className="rounded-lg border border-dashed px-4 py-8 text-center text-sm text-muted-foreground"
      >
        {t("entries.empty")}
      </p>
    );
  }
  return (
    <div className="overflow-x-auto rounded-lg border">
      <Table aria-label={t("entries.label")}>
        <TableHeader>
          <TableRow>
            <TableHead>{t("entries.item")}</TableHead>
            <TableHead className="hidden md:table-cell">{t("entries.type")}</TableHead>
            <TableHead className="hidden lg:table-cell">{t("entries.locale")}</TableHead>
            <TableHead>{t("entries.actions")}</TableHead>
            <TableHead className="hidden sm:table-cell">{t("entries.review")}</TableHead>
            {editable ? (
              <TableHead>
                <span className="sr-only">{t("entries.remove", { title: "" })}</span>
              </TableHead>
            ) : null}
          </TableRow>
        </TableHeader>
        <TableBody>
          {entries.map((e) => {
            const a = entryActions(e);
            const title = e.title || t("entries.untitled");
            return (
              <TableRow
                key={e.item_id}
                data-slot="entry-row"
                data-conflict={e.conflict || undefined}
              >
                <TableCell className="max-w-0 min-w-40 py-2.5">
                  <div className="flex min-w-0 flex-col gap-1">
                    <Link
                      to="/content/$type/$id"
                      params={{ type: e.type, id: e.item_id }}
                      {...(e.has_changes ? { search: { release: releaseId } } : {})}
                      className="truncate font-medium text-foreground underline-offset-4 hover:underline"
                    >
                      {title}
                    </Link>
                    {e.conflict ? (
                      <span className="flex flex-wrap items-center gap-2">
                        <span
                          data-slot="entry-conflict"
                          className="inline-flex h-6 items-center gap-1 rounded-md border border-warning/50 bg-warning/8 px-2 text-xs font-medium text-foreground"
                        >
                          <TriangleAlertIcon aria-hidden="true" className="size-3.5 text-warning" />
                          {t("entries.conflict")}
                        </span>
                        {editable ? (
                          <Tooltip>
                            <TooltipTrigger asChild>
                              <Button
                                type="button"
                                variant="outline"
                                size="sm"
                                disabled={busy}
                                onClick={() => onRebase(e)}
                              >
                                <GitMergeIcon data-icon="inline-start" />
                                {t("entries.rebase")}
                              </Button>
                            </TooltipTrigger>
                            <TooltipContent className="max-w-xs">
                              {t("entries.rebaseHint")}
                            </TooltipContent>
                          </Tooltip>
                        ) : null}
                      </span>
                    ) : null}
                  </div>
                </TableCell>
                <TableCell className="hidden text-sm text-muted-foreground md:table-cell">
                  {e.type}
                </TableCell>
                <TableCell className="hidden font-mono text-xs text-muted-foreground lg:table-cell">
                  {e.locale}
                </TableCell>
                <TableCell>
                  <div className="flex flex-wrap gap-1">
                    {a.changes ? <Chip kind="changes" /> : null}
                    {a.publish ? <Chip kind="publish" /> : null}
                    {a.unpublish ? <Chip kind="unpublish" /> : null}
                  </div>
                </TableCell>
                <TableCell className="hidden text-sm text-muted-foreground sm:table-cell">
                  {e.review ? (
                    <span data-slot="entry-review">
                      {e.review.status === "approved"
                        ? e.review.current
                          ? t("review.approved")
                          : t("review.stale")
                        : t("review.in_review", {
                            step: e.review.step + 1,
                            steps: e.review.steps,
                          })}
                    </span>
                  ) : (
                    t("entries.noReview")
                  )}
                </TableCell>
                {editable ? (
                  <TableCell className="w-12 text-right">
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon-sm"
                      aria-label={t("entries.remove", { title })}
                      disabled={busy}
                      onClick={() => onRemove(e)}
                      className="text-muted-foreground"
                    >
                      <XIcon />
                    </Button>
                  </TableCell>
                ) : null}
              </TableRow>
            );
          })}
        </TableBody>
      </Table>
    </div>
  );
}
