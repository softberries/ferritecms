// The cue for figures or rows that are about to change (M4-4 R-c3, like the palette's
// P81): a 2 px ember line, pulsing (still under reduced motion), along the top of what is
// being refreshed. The rows themselves keep full colour: dimmed to 60 %, muted text fell
// to about 2:1 in light mode. The container says it to assistive tech with `aria-busy`;
// this line is decoration.

import { cn } from "cn";

export interface StaleBarProps {
  /**
   * `sticky`: holds to the top of the nearest scroll container (a table frame) or below
   * the shell's top bar (a page section); `absolute`: across the top of a positioned box.
   */
  position?: "sticky" | "absolute";
  className?: string;
}

export function StaleBar({ position = "absolute", className }: StaleBarProps) {
  const line = (
    <span className="absolute inset-x-0 top-0 h-0.5 animate-pulse rounded-full bg-primary" />
  );
  return position === "sticky" ? (
    <div
      aria-hidden="true"
      data-slot="stale-bar"
      className={cn("pointer-events-none sticky top-0 z-20 h-0", className)}
    >
      {line}
    </div>
  ) : (
    <div
      aria-hidden="true"
      data-slot="stale-bar"
      className={cn("pointer-events-none absolute inset-x-0 top-0 z-10 h-0.5", className)}
    >
      {line}
    </div>
  );
}
