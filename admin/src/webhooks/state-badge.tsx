// An endpoint's state as a dot plus words, never colour alone (11 §3): Active, Failing
// since …, Disabled (with the reason), Paused: copied.

import { cn } from "cn";
import { formatDistanceToNowStrict } from "date-fns";
import { useTranslation } from "react-i18next";
import type { WebhookDto } from "@/api/webhooks";

const TONE: Record<WebhookDto["state"], string> = {
  active: "text-success",
  failing: "text-warning",
  disabled: "text-muted-foreground",
  paused: "text-warning",
};

export function WebhookStateBadge({ hook, className }: { hook: WebhookDto; className?: string }) {
  const { t } = useTranslation("webhooks");
  const label =
    hook.state === "failing" && hook.failing_since
      ? t("state.failingSince", {
          when: formatDistanceToNowStrict(new Date(hook.failing_since), { addSuffix: true }),
        })
      : t(`state.${hook.state}`);
  return (
    <span
      data-slot="webhook-state"
      data-state={hook.state}
      className={cn(
        "inline-flex h-6 shrink-0 items-center gap-1.5 rounded-full border px-2 text-xs font-medium whitespace-nowrap",
        TONE[hook.state],
        className,
      )}
    >
      <span
        aria-hidden="true"
        className={cn(
          "size-2 shrink-0 rounded-full border-[1.5px] border-current",
          hook.state === "disabled" ? "bg-transparent" : "bg-current",
        )}
      />
      {label}
    </span>
  );
}

/** Why a disabled endpoint is disabled, as text (nothing for other states). */
export function DisabledReason({ hook, className }: { hook: WebhookDto; className?: string }) {
  const { t } = useTranslation("webhooks");
  if (hook.state !== "disabled" || !hook.disabled_reason) return null;
  return (
    <span data-slot="disabled-reason" className={cn("text-xs text-muted-foreground", className)}>
      {t(`reason.${hook.disabled_reason}`)}
    </span>
  );
}
