// The selection bar: floats at the bottom of the screen while rows are selected (Linear,
// Stripe). It names the count and offers each bulk action the selection allows, with how
// many items it will touch when that is fewer than the selection ("Publish 2"). Esc
// clears the selection. It slides 8 px and fades (11 §5); under reduced motion it fades.

import { AnimatePresence, useReducedMotion } from "motion/react";
import * as m from "motion/react-m";
import { RotateCcwIcon, SendIcon, Trash2Icon, XCircleIcon, XIcon } from "lucide-react";
import { useLayoutEffect, useState, type CSSProperties, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { KbdCombo } from "@/components/kit/kbd-combo";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { MOTION } from "@/design/tokens";
import { uiLocale } from "@/lib/locale";
import { useShortcut } from "@/lib/shortcuts";
import { type BulkAction, type BulkItem, countAllowed } from "./bulk";

export interface BulkBarProps {
  /** Content items, or anything with their `allowed` (media items, `media/rights.ts`). */
  selected: readonly BulkItem[];
  pending: BulkAction | "undo" | null;
  onAction: (action: BulkAction) => void;
  onClear: () => void;
}

const ICONS: Record<BulkAction, ReactNode> = {
  publish: <SendIcon />,
  restore: <RotateCcwIcon />,
  trash: <Trash2Icon />,
  delete: <XCircleIcon />,
};
const ORDER: BulkAction[] = ["publish", "restore", "trash", "delete"];

/**
 * The horizontal centre of the shell's `<main>` in viewport pixels (it moves when the
 * sidebar collapses or the window resizes), so the fixed bar centres on the content,
 * not on the viewport. `null` before it is measured (or outside the shell).
 */
function useMainCentre(active: boolean): number | null {
  const [centre, setCentre] = useState<number | null>(null);
  useLayoutEffect(() => {
    if (!active) return;
    const main = document.getElementById("main");
    if (!main) return;
    const measure = () => {
      const r = main.getBoundingClientRect();
      setCentre(Math.round(r.left + r.width / 2));
    };
    measure();
    const ro = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(measure);
    ro?.observe(main);
    window.addEventListener("resize", measure);
    return () => {
      ro?.disconnect();
      window.removeEventListener("resize", measure);
    };
  }, [active]);
  return centre;
}

export function BulkBar({ selected, pending, onAction, onClear }: BulkBarProps) {
  const { t } = useTranslation("content");
  const reduce = useReducedMotion() ?? false;
  const open = selected.length > 0;
  const number = new Intl.NumberFormat(uiLocale());
  const centre = useMainCentre(open);

  useShortcut({
    id: "list.clearSelection",
    keys: "esc",
    label: t("shortcuts.clearSelection"),
    group: t("shortcuts.group"),
    // Not while a menu or dialog is open: Esc closes that first.
    when: () => open && document.querySelector("[role=menu], [role=dialog]") === null,
    run: onClear,
  });

  return (
    <AnimatePresence>
      {open ? (
        <m.div
          key="bulk-bar"
          role="region"
          aria-label={t("bulk.label")}
          data-slot="bulk-bar"
          initial={reduce ? { opacity: 0 } : { opacity: 0, y: 8 }}
          animate={reduce ? { opacity: 1 } : { opacity: 1, y: 0 }}
          exit={reduce ? { opacity: 0 } : { opacity: 0, y: 8 }}
          transition={{ duration: MOTION.durBase, ease: MOTION.easeOut }}
          className="fixed inset-x-3 bottom-3 z-40 flex items-center gap-1 rounded-xl border bg-popover p-1.5 text-popover-foreground shadow-3 sm:inset-x-auto sm:bottom-6 sm:left-(--bulk-bar-x) sm:-translate-x-1/2"
          style={{ "--bulk-bar-x": centre === null ? "50%" : `${centre}px` } as CSSProperties}
        >
          <p
            className="flex items-center gap-2 px-2 text-sm font-medium whitespace-nowrap tabular-nums"
            aria-live="polite"
          >
            <span
              aria-hidden="true"
              className="flex h-6 min-w-6 items-center justify-center rounded-md bg-primary px-1.5 text-xs text-primary-foreground"
            >
              {number.format(selected.length)}
            </span>
            <span aria-hidden="true">{t("bulk.selectedShort")}</span>
            <span className="sr-only">{t("bulk.selected", { count: selected.length })}</span>
          </p>
          <span aria-hidden="true" className="mx-1 h-5 w-px bg-border" />
          <div className="flex min-w-0 flex-1 items-center gap-1">
            {ORDER.map((action) => {
              const n = countAllowed(selected, action);
              if (n === 0) return null;
              const label =
                n === selected.length
                  ? t(`bulk.${action}`)
                  : t(`bulk.${action}Some`, { count: n, formatted: number.format(n) });
              return (
                <Button
                  key={action}
                  variant="ghost"
                  disabled={pending !== null}
                  aria-busy={pending === action || undefined}
                  aria-label={label}
                  onClick={() => onAction(action)}
                  className={
                    action === "trash" || action === "delete"
                      ? "text-destructive hover:text-destructive"
                      : undefined
                  }
                >
                  {ICONS[action]}
                  <span className="max-sm:sr-only">{label}</span>
                </Button>
              );
            })}
          </div>
          <Tooltip>
            <TooltipTrigger asChild>
              <Button variant="ghost" size="icon" aria-label={t("bulk.clear")} onClick={onClear}>
                <XIcon />
              </Button>
            </TooltipTrigger>
            <TooltipContent>
              {t("bulk.clear")} <KbdCombo keys="esc" />
            </TooltipContent>
          </Tooltip>
        </m.div>
      ) : null}
    </AnimatePresence>
  );
}
