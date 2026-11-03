// A release's status as a dot and a label (never colour alone, 11 §3), in the content
// status tones: open like a draft, scheduled, applied like published, failed in red.

import { cn } from "cn";
import { useTranslation } from "react-i18next";
import { isReleaseStatus } from "@/api/releases";

const TONE: Record<string, string> = {
  open: "text-status-draft",
  scheduled: "text-status-scheduled",
  applied: "text-status-published",
  failed: "text-destructive",
};

export function ReleaseBadge({ status, className }: { status: string; className?: string }) {
  const { t } = useTranslation("releases");
  return (
    <span
      data-slot="release-badge"
      data-status={status}
      className={cn(
        "inline-flex h-6 shrink-0 items-center gap-1.5 rounded-full border px-2 text-xs font-medium whitespace-nowrap",
        TONE[status] ?? "text-muted-foreground",
        className,
      )}
    >
      <span
        aria-hidden="true"
        className={cn(
          "size-2 shrink-0 rounded-full border-[1.5px] border-current",
          status === "open" ? "bg-transparent" : "bg-current",
        )}
      />
      {isReleaseStatus(status) ? t(`status.${status}`) : status}
    </span>
  );
}
