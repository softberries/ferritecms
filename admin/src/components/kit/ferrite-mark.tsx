// The product mark (docs/brand/ferrite-mark-flat.svg): the hexagon "F" in ember and
// steel. Decorative; the product or site name always sits next to it as text.

import { cn } from "cn";

const EMBER = [
  "M72 10.62 64 6 13.77 35v27l13.86 8V43L64 22l8 4.62z",
  "M13.77 68v25l13.86-8v-9z",
  "M78 14.08 94.37 23.53v16L78 30.08z",
  "M32.65 75.77 78 49.6v16.17L46.65 83.86v28.12l-14-8.08z",
];
const STEEL = [
  "M100.37 26.99 114.23 35v58L64 122l-12-6.93V86.54l14-8.08v26.39L100.37 85V43z",
  "M32.65 45.9 62 28.96v16.17l-15.35 8.86v7.93l-14 8.08z",
];

export function FerriteMark({ className }: { className?: string }) {
  return (
    <svg
      aria-hidden="true"
      viewBox="0 0 128 128"
      className={cn("size-9 shrink-0", className)}
      data-testid="ferrite-mark"
    >
      <g fill="var(--brand-ember)">
        {EMBER.map((d) => (
          <path key={d} d={d} />
        ))}
      </g>
      <g fill="var(--brand-steel)">
        {STEEL.map((d) => (
          <path key={d} d={d} />
        ))}
      </g>
    </svg>
  );
}
