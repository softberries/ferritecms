// A determinate progress bar (uploads): `value` from 0 to 1, a percent for assistive
// technology, the fill scaled with a transform (11 §5: `--dur-base`, none under reduced
// motion). `className` replaces the default height (`h-1`) when it sets one.

import { cn } from "cn";

export interface ProgressBarProps {
  /** From 0 to 1. */
  value: number;
  /** The bar's accessible name. */
  label: string;
  className?: string;
}

export function ProgressBar({ value, label, className }: ProgressBarProps) {
  const percent = Math.round(value * 100);
  return (
    <div
      role="progressbar"
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={percent}
      className={cn("h-1 w-full overflow-hidden rounded-full bg-muted", className)}
    >
      <div
        className="h-full origin-left rounded-full bg-primary transition-transform duration-(--dur-base) ease-out motion-reduce:transition-none"
        style={{ transform: `scaleX(${value})` }}
      />
    </div>
  );
}
