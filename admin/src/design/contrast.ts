// Colour maths for the token contrast gate (docs/design/11-admin-ui.md §3).
//
// `tokens.css` defines every colour as `oklch(L C H)` or `oklch(L C H / A)`, or as
// `var(--other)`. `parseTokens` reads the `:root` (light) and `.dark` blocks; `.dark`
// sits on `<html>`, the same element as `:root`, so a `var()` reference resolves in the
// mode it is read in. Ratios follow WCAG 2.x (relative luminance of gamma-encoded sRGB).

export type Mode = "light" | "dark";

/** An OKLCH colour; `alpha` is 1 for an opaque colour. */
export interface Oklch {
  l: number;
  c: number;
  h: number;
  alpha: number;
}

/** Gamma-encoded sRGB, each channel in 0..1. */
export interface Srgb {
  r: number;
  g: number;
  b: number;
}

const OKLCH = /^oklch\(\s*([\d.]+)\s+([\d.]+)\s+([\d.]+)\s*(?:\/\s*([\d.]+)(%?)\s*)?\)$/;
const VAR = /^var\(\s*(--[\w-]+)\s*\)$/;

/** The custom properties of the first `<selector> { … }` block, in source order. */
function block(css: string, selector: string): Map<string, string> {
  const start = css.search(new RegExp(`(^|\\n)${selector.replace(".", "\\.")}\\s*\\{`));
  const out = new Map<string, string>();
  if (start < 0) return out;
  const open = css.indexOf("{", start);
  const close = css.indexOf("}", open);
  const body = css.slice(open + 1, close).replace(/\/\*[\s\S]*?\*\//g, "");
  for (const decl of body.split(";")) {
    const m = /^\s*(--[\w-]+)\s*:\s*([\s\S]+?)\s*$/.exec(decl);
    if (m?.[1] && m[2]) out.set(m[1].slice(2), m[2].replace(/\s+/g, " "));
  }
  return out;
}

function parseColour(value: string): Oklch | undefined {
  const m = OKLCH.exec(value);
  if (!m) return undefined;
  const alpha = m[4] === undefined ? 1 : Number(m[4]) / (m[5] === "%" ? 100 : 1);
  return { l: Number(m[1]), c: Number(m[2]), h: Number(m[3]), alpha };
}

/**
 * The colour tokens of `tokens.css` per mode, `var()` references resolved. Tokens
 * that are not colours (radius, shadows, durations) are left out.
 */
export function parseTokens(css: string): Record<Mode, Record<string, Oklch>> {
  const light = block(css, ":root");
  const dark = new Map([...light, ...block(css, ".dark")]);
  const resolve = (vars: Map<string, string>): Record<string, Oklch> => {
    const out: Record<string, Oklch> = {};
    for (const name of vars.keys()) {
      let value = vars.get(name);
      for (let hops = 0; value !== undefined && hops < 8; hops++) {
        const ref = VAR.exec(value);
        if (!ref?.[1]) break;
        value = vars.get(ref[1].slice(2));
      }
      const colour = value === undefined ? undefined : parseColour(value);
      if (colour) out[name] = colour;
    }
    return out;
  };
  return { light: resolve(light), dark: resolve(dark) };
}

/** OKLCH to linear sRGB (Björn Ottosson's OKLab matrices). */
function oklchToLinear({ l, c, h }: Oklch): [number, number, number] {
  const rad = (h * Math.PI) / 180;
  const a = c * Math.cos(rad);
  const b = c * Math.sin(rad);
  const l_ = (l + 0.3963377774 * a + 0.2158037573 * b) ** 3;
  const m_ = (l - 0.1055613458 * a - 0.0638541728 * b) ** 3;
  const s_ = (l - 0.0894841775 * a - 1.291485548 * b) ** 3;
  return [
    4.0767416621 * l_ - 3.3077115913 * m_ + 0.2309699292 * s_,
    -1.2684380046 * l_ + 2.6097574011 * m_ - 0.3413193965 * s_,
    -0.0041960863 * l_ - 0.7034186147 * m_ + 1.707614701 * s_,
  ];
}

const encode = (x: number) => (x <= 0.0031308 ? 12.92 * x : 1.055 * x ** (1 / 2.4) - 0.055);
const decode = (x: number) => (x <= 0.04045 ? x / 12.92 : ((x + 0.055) / 1.055) ** 2.4);
const clamp = (x: number) => Math.min(1, Math.max(0, x));

/** Whether the colour is inside the sRGB gamut (no browser gamut mapping involved). */
export function inSrgbGamut(colour: Oklch, epsilon = 1e-3): boolean {
  return oklchToLinear(colour).every((x) => x >= -epsilon && x <= 1 + epsilon);
}

/** Gamma-encoded sRGB, clamped to the gamut; alpha is ignored (see `over`). */
export function oklchToSrgb(colour: Oklch): Srgb {
  const [r, g, b] = oklchToLinear(colour).map((x) => clamp(encode(x))) as [number, number, number];
  return { r, g, b };
}

/** `fg` composited over the opaque `bg`, in gamma-encoded sRGB as browsers blend. */
export function over(fg: Oklch, bg: Oklch): Srgb {
  const f = oklchToSrgb(fg);
  const b = oklchToSrgb(bg);
  const a = fg.alpha;
  return { r: f.r * a + b.r * (1 - a), g: f.g * a + b.g * (1 - a), b: f.b * a + b.b * (1 - a) };
}

/** WCAG 2.x relative luminance. */
export function relativeLuminance({ r, g, b }: Srgb): number {
  return 0.2126 * decode(r) + 0.7152 * decode(g) + 0.0722 * decode(b);
}

/** WCAG 2.x contrast ratio, 1..21, symmetric in its arguments. */
export function contrastRatio(a: Srgb, b: Srgb): number {
  const la = relativeLuminance(a);
  const lb = relativeLuminance(b);
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
}
