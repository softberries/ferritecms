// A shortcut as keycaps: `mod+k`, `mod+shift+p`, a sequence `g d` (11 §6).
// `mod` is ⌘ on Apple platforms and Ctrl elsewhere.

import { Fragment } from "react";
import { useTranslation } from "react-i18next";
import { Kbd, KbdGroup } from "@/components/ui/kbd";
import { isApplePlatform } from "@/lib/shortcuts";

/** Visible cap and spoken name of one key. */
function keyCap(key: string, apple: boolean): { cap: string; name: string } {
  switch (key.toLowerCase()) {
    case "mod":
      return apple ? { cap: "⌘", name: "Command" } : { cap: "Ctrl", name: "Control" };
    case "shift":
      return apple ? { cap: "⇧", name: "Shift" } : { cap: "Shift", name: "Shift" };
    case "alt":
      return apple ? { cap: "⌥", name: "Option" } : { cap: "Alt", name: "Alt" };
    case "enter":
      return { cap: "↵", name: "Enter" };
    case "esc":
      return { cap: "Esc", name: "Escape" };
    case "up":
      return { cap: "↑", name: "Up" };
    case "down":
      return { cap: "↓", name: "Down" };
    default:
      return { cap: key.length === 1 ? key.toUpperCase() : key, name: key.toUpperCase() };
  }
}

export interface KbdComboProps {
  /** `+` joins keys pressed together; a space separates steps of a sequence. */
  keys: string;
  className?: string;
}

export function KbdCombo({ keys, className }: KbdComboProps) {
  const { t } = useTranslation();
  const apple = isApplePlatform();
  const steps = keys
    .trim()
    .split(/\s+/)
    .map((step) => step.split("+").map((k) => keyCap(k, apple)));
  const spoken = steps
    .map((step) => step.map((k) => k.name).join("+"))
    .join(` ${t("kit.kbd.then")} `);
  return (
    <KbdGroup data-slot="kbd-combo" className={className}>
      <span className="sr-only">{spoken}</span>
      {steps.map((step, i) => (
        <Fragment key={i}>
          {i > 0 ? (
            <span aria-hidden="true" className="text-xs text-muted-foreground">
              {t("kit.kbd.then")}
            </span>
          ) : null}
          {step.map((k, j) => (
            <Kbd key={j} aria-hidden="true">
              {k.cap}
            </Kbd>
          ))}
        </Fragment>
      ))}
    </KbdGroup>
  );
}
