// The editor's save indicator. The polite live region carries plain text, so a screen
// reader hears each change once; the visible part crossfades (11 §5, ≤ 120 ms). Quiet by
// design (after Linear and Notion): the text stays muted while things are fine, only the
// icon carries a tone for unsaved changes; failures and offline are coloured. `autosaved`
// means the draft is safe in the autosave slot but the item itself is not updated yet.
// Below `sm` the visible label is the short form, so a narrow bar never overflows.

import { format } from "date-fns";
import {
  CircleAlertIcon,
  CircleCheckIcon,
  CircleDotIcon,
  CloudCheckIcon,
  CloudOffIcon,
  LoaderCircleIcon,
} from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { cn } from "cn";
import { useTranslation } from "react-i18next";

export type SaveStateValue = "saved" | "autosaved" | "saving" | "dirty" | "error" | "offline";

export interface SaveStateProps {
  state: SaveStateValue;
  /** When the last save landed (shown with `saved` and `autosaved`). */
  at?: Date;
  className?: string;
}

const ICON = {
  saved: CircleCheckIcon,
  autosaved: CloudCheckIcon,
  saving: LoaderCircleIcon,
  dirty: CircleDotIcon,
  error: CircleAlertIcon,
  offline: CloudOffIcon,
} as const;

const TONE: Record<SaveStateValue, string> = {
  saved: "text-muted-foreground",
  autosaved: "text-muted-foreground",
  saving: "text-muted-foreground",
  dirty: "text-muted-foreground",
  error: "text-destructive",
  offline: "text-warning",
};

/** The icon's own tone where it differs from the text's. */
const ICON_TONE: Partial<Record<SaveStateValue, string>> = { dirty: "text-warning" };

export function SaveState({ state, at, className }: SaveStateProps) {
  const { t } = useTranslation();
  const text =
    state === "saved" && at
      ? t("kit.save.savedAt", { time: format(at, "p") })
      : state === "autosaved" && at
        ? t("kit.save.autosavedAt", { time: format(at, "p") })
        : t(`kit.save.${state}`);
  const time = at ? format(at, "p") : null;
  const short =
    (state === "saved" || state === "autosaved") && time
      ? t(`kit.save.short.${state}At`, { time })
      : t(`kit.save.short.${state}`);
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
          <Icon className={cn("size-4", ICON_TONE[state], state === "saving" && "animate-spin")} />
          {/* Phone width: the short form ("Offline", "Autosaved 10:42"). */}
          <span className="max-sm:hidden">{text}</span>
          <span className="sm:hidden">{short}</span>
        </motion.span>
      </AnimatePresence>
    </span>
  );
}
