// Cells of the content list that are components of their own (fast refresh needs them in
// a module that exports only components).

import { format } from "date-fns";
import { useTranslation } from "react-i18next";
import { isContentStatus } from "@/api/content";
import type { SchemaContentSummaryDto } from "@/api/schema";
import { StatusBadge } from "@/components/kit/status-badge";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";

/** The status badge, with the scheduled time under it for scheduled items. */
export function StatusCell({ item }: { item: SchemaContentSummaryDto }) {
  const { t } = useTranslation("content");
  if (!isContentStatus(item.status)) return <span>{item.status}</span>;
  return (
    <div className="flex flex-col items-start gap-0.5 py-1.5">
      <StatusBadge status={item.status} />
      {item.status === "scheduled" && item.scheduled_at ? (
        <time
          dateTime={item.scheduled_at}
          className="text-xs text-muted-foreground tabular-nums"
          title={format(new Date(item.scheduled_at), "PPpp")}
        >
          {t("row.scheduledFor", { at: format(new Date(item.scheduled_at), "PP p") })}
        </time>
      ) : null}
    </div>
  );
}

/**
 * A parent the viewer may not read (someone else's draft above a published child): a
 * muted label, with why in a tooltip (and for screen readers in the label itself).
 */
export function UnavailableParent({ className }: { className?: string }) {
  const { t } = useTranslation("content");
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span
          tabIndex={0}
          data-slot="parent-unavailable"
          className={className ?? "text-muted-foreground italic"}
        >
          {t("row.parentUnavailable")}
          <span className="sr-only">. {t("row.parentUnavailableWhy")}</span>
        </span>
      </TooltipTrigger>
      <TooltipContent>{t("row.parentUnavailableWhy")}</TooltipContent>
    </Tooltip>
  );
}
