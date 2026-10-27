// Token names and the contrast pairs the gate checks (docs/design/11-admin-ui.md §3–§5).
// Values live only in `src/styles/tokens.css`.

import type { Mode } from "./contrast";

/** Every token `tokens.css` defines, by group (names without the leading `--`). */
export const TOKENS = {
  surface: [
    "background",
    "foreground",
    "card",
    "card-foreground",
    "popover",
    "popover-foreground",
    "muted",
    "muted-foreground",
    "secondary",
    "secondary-foreground",
    "accent",
    "accent-foreground",
    "border",
    "input",
    "ring",
    "overlay",
  ],
  brand: ["primary", "primary-foreground", "primary-hover"],
  semantic: ["destructive", "success", "warning", "info"],
  status: [
    "status-draft",
    "status-pending",
    "status-scheduled",
    "status-published",
    "status-private",
    "status-trash",
  ],
  chart: ["chart-1", "chart-2", "chart-3", "chart-4", "chart-5"],
  /** Fixed black on white in both modes (QR codes). */
  fixed: ["qr-fg", "qr-bg"],
  sidebar: [
    "sidebar",
    "sidebar-foreground",
    "sidebar-primary",
    "sidebar-primary-foreground",
    "sidebar-accent",
    "sidebar-accent-foreground",
    "sidebar-border",
    "sidebar-ring",
  ],
  radius: ["radius"],
  shadow: ["shadow-1", "shadow-2", "shadow-3"],
  duration: ["dur-instant", "dur-fast", "dur-base", "dur-slow"],
  easing: ["ease-out", "ease-in-out"],
  density: ["control-h", "row-h", "page-pad"],
} as const;

/** The colour tokens (every group but radius, shadow, motion and density). */
export const COLOUR_TOKENS: readonly string[] = [
  ...TOKENS.surface,
  ...TOKENS.brand,
  ...TOKENS.semantic,
  ...TOKENS.status,
  ...TOKENS.chart,
  ...TOKENS.sidebar,
  ...TOKENS.fixed,
];

export interface ContrastPair {
  fg: string;
  bg: string;
  /** 4.5 for text (WCAG 1.4.3), 3 for UI boundaries and graphics (WCAG 1.4.11). */
  min: 4.5 | 3;
  modes: Mode[];
}

const BOTH: Mode[] = ["light", "dark"];
const on = (fgs: readonly string[], bgs: readonly string[], min: 4.5 | 3): ContrastPair[] =>
  fgs.flatMap((fg) => bgs.map((bg) => ({ fg, bg, min, modes: BOTH })));

/** Every pair the token gate checks, in both modes. */
export const CONTRAST_PAIRS: ContrastPair[] = [
  ...on(["foreground", "muted-foreground"], ["background", "card", "muted"], 4.5),
  ...on(["primary-foreground"], ["primary", "primary-hover"], 4.5),
  // `primary` is also link text.
  ...on(["primary"], ["background", "card"], 4.5),
  ...on([...TOKENS.semantic, ...TOKENS.status], ["background", "card"], 4.5),
  ...on(["sidebar-foreground"], ["sidebar", "sidebar-accent"], 4.5),
  ...on(["input", "ring"], ["background"], 3),
  ...on(TOKENS.chart, ["background", "card"], 3),
  // A QR code's modules on its own ground (the quiet zone), the same in both modes.
  ...on(["qr-fg"], ["qr-bg"], 4.5),
];

/**
 * `--dur-base` and `--ease-out` as numbers, for the `motion` package (seconds, a
 * cubic-bezier array). `contrast.test.ts` keeps them equal to tokens.css (11 §5).
 */
export const MOTION = {
  durBase: 0.18,
  easeOut: [0.22, 1, 0.36, 1] as [number, number, number, number],
} as const;
