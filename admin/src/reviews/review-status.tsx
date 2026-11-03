// A review's state as a dot and a label (never colour alone, 11 §3).

import { cn } from "cn";
import { useTranslation } from "react-i18next";

const TONE: Record<string, string> = {
  in_review: "text-status-pending",
  approved: "text-status-published",
  changes_requested: "text-destructive",
  withdrawn: "text-muted-foreground",
  stale: "text-muted-foreground",
};

const KNOWN = ["in_review", "approved", "changes_requested", "withdrawn", "stale"] as const;

export function ReviewStatus({ status, className }: { status: string; className?: string }) {
  const { t } = useTranslation("reviews");
  const known = KNOWN.find((k) => k === status);
  return (
    <span
      data-slot="review-status"
      data-status={status}
      className={cn(
        "inline-flex h-6 shrink-0 items-center gap-1.5 rounded-full border px-2 text-xs font-medium whitespace-nowrap",
        TONE[status] ?? "text-muted-foreground",
        className,
      )}
    >
      <span aria-hidden="true" className="size-2 shrink-0 rounded-full bg-current" />
      {known ? t(`status.${known}`) : status}
    </span>
  );
}
