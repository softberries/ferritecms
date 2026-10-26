// The look of a row of link tabs with counts (the content list's status tabs, the
// comments queue's): a tinted underline on the current tab, a pill with the count.

import { cn } from "cn";

/** The `<ul>` of the tabs: the rule under them is an inset shadow, so the active
 * underline covers it inside the scrolling row. */
export const TAB_LIST = "flex w-max min-w-full gap-1 shadow-[inset_0_-1px_0_var(--border)]";

export function tabLinkClass(active: boolean): string {
  return cn(
    "relative flex h-control items-center gap-2 rounded-t-md px-2.5 text-sm font-medium whitespace-nowrap -outline-offset-2 transition-colors",
    "after:absolute after:inset-x-1.5 after:bottom-0 after:h-0.5 after:rounded-full after:transition-colors",
    active
      ? "text-foreground after:bg-primary"
      : "text-muted-foreground after:bg-transparent hover:text-foreground hover:after:bg-border",
  );
}

export function tabCountClass(active: boolean): string {
  return cn(
    "min-w-5 rounded-full px-1.5 text-center text-xs leading-5 tabular-nums",
    active ? "bg-primary/10 text-foreground" : "bg-muted text-muted-foreground",
  );
}
