// The token gate (docs/design/11-admin-ui.md §3, §5, §8). Reads the CSS as text through
// Vite's `?raw` imports, so no Node API is needed in the jsdom project.

import { describe, expect, it } from "vitest";
import globalsCss from "../styles/globals.css?raw";
import tokensCss from "../styles/tokens.css?raw";
import {
  contrastRatio,
  inSrgbGamut,
  oklchToSrgb,
  over,
  parseTokens,
  type Mode,
  type Srgb,
} from "./contrast";
import { COLOUR_TOKENS, CONTRAST_PAIRS, TOKENS } from "./tokens";

const MODES: Mode[] = ["light", "dark"];
const tokens = parseTokens(tokensCss);

describe("design tokens", () => {
  it("every_token_is_defined", () => {
    for (const mode of MODES) {
      const missing = COLOUR_TOKENS.filter((name) => !(name in tokens[mode]));
      expect(missing, `colour tokens missing in ${mode}`).toEqual([]);
    }
    const others = [
      ...TOKENS.radius,
      ...TOKENS.shadow,
      ...TOKENS.duration,
      ...TOKENS.easing,
      ...TOKENS.density,
    ];
    const missing = others.filter((name) => !new RegExp(`--${name}\\s*:`).test(tokensCss));
    expect(missing, "non-colour tokens missing").toEqual([]);
  });

  it("every_contrast_pair_meets_its_minimum", () => {
    const failures: string[] = [];
    for (const pair of CONTRAST_PAIRS) {
      for (const mode of pair.modes) {
        const fg = tokens[mode][pair.fg];
        const bg = tokens[mode][pair.bg];
        if (!fg || !bg) {
          failures.push(`${mode}: ${pair.fg} on ${pair.bg} is not defined`);
          continue;
        }
        const ratio = contrastRatio(over(fg, bg), oklchToSrgb(bg));
        if (ratio < pair.min) {
          failures.push(`${mode}: ${pair.fg} on ${pair.bg} is ${ratio.toFixed(2)} < ${pair.min}`);
        }
      }
    }
    expect(failures).toEqual([]);
  });

  it("every_colour_token_is_in_srgb_gamut", () => {
    // Out-of-gamut colours are gamut-mapped by the browser, which would make the
    // ratios above approximate.
    for (const mode of MODES) {
      const outside = Object.entries(tokens[mode])
        .filter(([, colour]) => !inSrgbGamut(colour))
        .map(([name]) => name);
      expect(outside, `out of sRGB gamut in ${mode}`).toEqual([]);
    }
  });

  it("contrast_math_matches_known_values", () => {
    const grey = (v: number): Srgb => ({ r: v / 255, g: v / 255, b: v / 255 });
    expect(contrastRatio(grey(0), grey(255))).toBeCloseTo(21, 5);
    expect(contrastRatio(grey(255), grey(0))).toBeCloseTo(21, 5);
    // The textbook near-miss: 0x777777 on white is 4.48:1.
    expect(contrastRatio(grey(0x77), grey(255))).toBeCloseTo(4.48, 2);
    // OKLCH round trip: L 1 is white, L 0 is black.
    const white = oklchToSrgb({ l: 1, c: 0, h: 0, alpha: 1 });
    expect(white.r).toBeCloseTo(1, 3);
    expect(white.g).toBeCloseTo(1, 3);
    expect(white.b).toBeCloseTo(1, 3);
    const black = oklchToSrgb({ l: 0, c: 0, h: 0, alpha: 1 });
    expect(contrastRatio(black, white)).toBeCloseTo(21, 2);
    // Half-transparent black over white blends to 0x80 grey.
    const half = over({ l: 0, c: 0, h: 0, alpha: 0.5 }, { l: 1, c: 0, h: 0, alpha: 1 });
    expect(half.r * 255).toBeCloseTo(127.5, 0);
  });

  it("parse_tokens_resolves_references_per_mode", () => {
    const css = `:root {\n  --a: oklch(0.5 0.1 40);\n  --b: var(--a);\n  --c: oklch(0.2 0 0 / 70%);\n  --radius: 0.5rem;\n}\n.dark {\n  --a: oklch(0.8 0.1 40);\n}\n`;
    const parsed = parseTokens(css);
    expect(parsed.light.b).toEqual({ l: 0.5, c: 0.1, h: 40, alpha: 1 });
    expect(parsed.dark.b).toEqual({ l: 0.8, c: 0.1, h: 40, alpha: 1 });
    expect(parsed.dark.c?.alpha).toBeCloseTo(0.7, 5);
    expect(parsed.light.radius).toBeUndefined();
  });

  it("reduced_motion_zeroes_durations", () => {
    const media = /@media\s*\(prefers-reduced-motion:\s*reduce\)\s*\{([\s\S]*?)\n\}/;
    const tokenBlock = media.exec(tokensCss)?.[1] ?? "";
    for (const name of TOKENS.duration) {
      expect(tokenBlock, `--${name} under reduced motion`).toMatch(
        new RegExp(`--${name}\\s*:\\s*0ms\\s*;`),
      );
    }
    // Keyframe animations (tw-animate-css, used by Radix overlays) have their own
    // durations, so globals.css also shortens every animation and transition (F29).
    const globalBlock = media.exec(globalsCss)?.[1] ?? "";
    expect(globalBlock).toMatch(/\*,\s*::before,\s*::after\s*\{/);
    expect(globalBlock).toMatch(/animation-duration:\s*0\.01ms\s*!important/);
    expect(globalBlock).toMatch(/animation-iteration-count:\s*1\s*!important/);
    expect(globalBlock).toMatch(/transition-duration:\s*0\.01ms\s*!important/);
  });

  it("no_raw_colours_outside_tokens", () => {
    const files = import.meta.glob<string>("/src/**/*.{ts,tsx,css}", {
      query: "?raw",
      import: "default",
      eager: true,
    });
    // Selectors in the generated shadcn chart that target Recharts' default colours
    // (pre-flight F30; listed in 11 §7). Only these exact strings, only in that file;
    // built from parts so this file holds no literal itself.
    const allowed: Record<string, string[]> = {
      "/src/components/ui/chart.tsx": ["ccc", "fff"].map((c) => `[stroke='#${c}']`),
    };
    const literal = /#(?:[0-9a-fA-F]{8}|[0-9a-fA-F]{6}|[0-9a-fA-F]{3,4})\b|\b(?:rgba?|hsla?)\(/g;
    const found: string[] = [];
    expect(Object.keys(files).length).toBeGreaterThan(10);
    for (const [path, source] of Object.entries(files)) {
      if (path === "/src/styles/tokens.css") continue;
      let text = source;
      for (const selector of allowed[path] ?? []) text = text.split(selector).join("");
      for (const m of text.matchAll(literal)) found.push(`${path}: ${m[0]}`);
    }
    expect(found).toEqual([]);
  });
});
