// A content status as a dot plus a label, never colour alone (11 §3).

import { cn } from "cn";
import { useTranslation } from "react-i18next";
import type { ContentStatus } from "@/api/content";

const TONE: Record<ContentStatus, string> = {
  draft: "text-status-draft",
  pending: "text-status-pending",
  scheduled: "text-status-scheduled",
  published: "text-status-published",
  private: "text-status-private",
  trash: "text-status-trash",
};

export interface StatusBadgeProps {
  status: ContentStatus;
  className?: string;
}

export function StatusBadge({ status, className }: StatusBadgeProps) {
  const { t } = useTranslation();
  return (
    <span
      data-slot="status-badge"
      data-status={status}
      className={cn(
        "inline-flex h-6 shrink-0 items-center gap-1.5 rounded-full border px-2 text-xs font-medium whitespace-nowrap",
        TONE[status],
        className,
      )}
    >
      {/* A hollow dot for drafts, a filled one once the item has moved on. */}
      <span
        aria-hidden="true"
        className={cn(
          "size-2 rounded-full border-[1.5px] border-current",
          status === "draft" ? "bg-transparent" : "bg-current",
        )}
      />
      {t(`status.${status}`)}
    </span>
  );
}
