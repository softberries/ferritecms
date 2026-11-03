// The marks of a change set: its connection (an app's sparkle or a token's key, with
// the words, never an icon alone) and its status as a dot plus a label (11 §3).

import { cn } from "cn";
import { KeyRoundIcon, SparklesIcon } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { ChangeSetDto } from "@/api/change-sets";
import { statusOf } from "./model";

const TONE = {
  open: "text-success",
  closed: "text-muted-foreground",
  partly_reverted: "text-warning",
  reverted: "text-info",
} as const;

export function SetStatusBadge({ set, className }: { set: ChangeSetDto; className?: string }) {
  const { t } = useTranslation("ai");
  const status = statusOf(set);
  return (
    <span
      data-slot="set-status"
      data-status={status}
      className={cn(
        "inline-flex h-6 shrink-0 items-center gap-1.5 rounded-full border px-2 text-xs font-medium whitespace-nowrap",
        TONE[status],
        className,
      )}
    >
      <span
        aria-hidden="true"
        className={cn(
          "size-2 shrink-0 rounded-full border-[1.5px] border-current",
          status === "closed" ? "bg-transparent" : "bg-current",
        )}
      />
      {t(`activity.status.${status}`)}
    </span>
  );
}

export function AgentMark({ agent }: { agent: ChangeSetDto["agent"] }) {
  const { t } = useTranslation("ai");
  const Icon = agent.kind === "token" ? KeyRoundIcon : SparklesIcon;
  return (
    <span
      title={agent.kind === "token" ? t("activity.token") : undefined}
      className="flex size-9 shrink-0 items-center justify-center rounded-lg border bg-primary/8 text-primary"
    >
      <Icon aria-hidden="true" className="size-4" />
      {agent.kind === "token" ? <span className="sr-only">{t("activity.token")}</span> : null}
    </span>
  );
}
