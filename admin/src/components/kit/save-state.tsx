// The editor's save indicator. The polite live region carries plain text, so a screen
// reader hears each change once; the visible part crossfades (11 §5, ≤ 120 ms).

import { format } from "date-fns";
import {
  CircleAlertIcon,
  CircleCheckIcon,
  CircleDotIcon,
  CloudOffIcon,
  LoaderCircleIcon,
} from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { cn } from "cn";
import { useTranslation } from "react-i18next";

export type SaveStateValue = "saved" | "saving" | "dirty" | "error" | "offline";

export interface SaveStateProps {
  state: SaveStateValue;
  /** When the last save landed (shown with `saved`). */
  at?: Date;
  className?: string;
}

const ICON = {
  saved: CircleCheckIcon,
  saving: LoaderCircleIcon,
  dirty: CircleDotIcon,
  error: CircleAlertIcon,
  offline: CloudOffIcon,
} as const;

const TONE: Record<SaveStateValue, string> = {
  saved: "text-muted-foreground",
  saving: "text-muted-foreground",
  dirty: "text-warning",
  error: "text-destructive",
  offline: "text-warning",
};

export function SaveState({ state, at, className }: SaveStateProps) {
  const { t } = useTranslation();
  const text =
    state === "saved" && at
      ? t("kit.save.savedAt", { time: format(at, "p") })
      : t(`kit.save.${state}`);
  const Icon = ICON[state];
  return (
    <span
      data-slot="save-state"
      data-state={state}
      className={cn("relative inline-flex h-6 items-center text-sm font-medium", className)}
    >
      <span role="status" aria-live="polite" className="sr-only">
        {text}
      </span>
      <AnimatePresence mode="wait" initial={false}>
        <motion.span
          key={`${state}:${text}`}
          aria-hidden="true"
          className={cn("inline-flex items-center gap-1.5 whitespace-nowrap", TONE[state])}
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={{ duration: 0.12, ease: [0.22, 1, 0.36, 1] }}
        >
          <Icon className={cn("size-4", state === "saving" && "animate-spin")} />
          {text}
        </motion.span>
      </AnimatePresence>
    </span>
  );
}
