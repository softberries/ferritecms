// The `?` sheet: every binding registered right now (`listShortcuts()`), by group.

import { useTranslation } from "react-i18next";
import { KbdCombo } from "@/components/kit/kbd-combo";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { listShortcuts, type Shortcut } from "@/lib/shortcuts";

function byGroup(shortcuts: Shortcut[]): [string, Shortcut[]][] {
  const groups = new Map<string, Shortcut[]>();
  for (const s of shortcuts) groups.set(s.group, [...(groups.get(s.group) ?? []), s]);
  return [...groups.entries()];
}

export interface ShortcutSheetProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

export function ShortcutSheet({ open, onOpenChange }: ShortcutSheetProps) {
  const { t } = useTranslation("shell");
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        className="max-h-[85svh] gap-5 overflow-y-auto sm:max-w-2xl"
        // A reference sheet with nothing to fill in: focus the sheet itself (Escape
        // closes it), not the close button, which would open with a focus ring.
        onOpenAutoFocus={(event) => {
          event.preventDefault();
          if (event.currentTarget instanceof HTMLElement) event.currentTarget.focus();
        }}
      >
        <DialogHeader>
          <DialogTitle>{t("shortcuts.title")}</DialogTitle>
          <DialogDescription>{t("shortcuts.description")}</DialogDescription>
        </DialogHeader>
        {/* Read at open time: what is registered on this screen. */}
        {open ? (
          <div className="grid gap-x-8 gap-y-6 sm:grid-cols-2">
            {byGroup(listShortcuts()).map(([group, items], i) => (
              <section key={group} aria-labelledby={`shortcut-group-${i}`}>
                <h3
                  id={`shortcut-group-${i}`}
                  className="mb-1.5 text-xs font-medium tracking-wide text-muted-foreground uppercase"
                >
                  {group}
                </h3>
                <dl className="divide-y">
                  {items.map((s) => (
                    <div key={s.id} className="flex items-center justify-between gap-4 py-2">
                      <dt className="text-sm text-foreground">{s.label}</dt>
                      <dd className="shrink-0">
                        <KbdCombo keys={s.keys} />
                      </dd>
                    </div>
                  ))}
                </dl>
              </section>
            ))}
          </div>
        ) : null}
      </DialogContent>
    </Dialog>
  );
}
