// Foundations: colour tokens with their contrast, type, spacing/radius/elevation/density,
// motion (11 §2–§5). Values come from tokens.css itself, so this page cannot drift.

import { CheckIcon, PlayIcon } from "lucide-react";
import { cn } from "cn";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { contrastRatio, oklchToSrgb, over, parseTokens, type Mode } from "@/design/contrast";
import { useDensity } from "@/design/density";
import { useTheme } from "@/design/theme";
import { CONTRAST_PAIRS, TOKENS } from "@/design/tokens";
import tokensCss from "@/styles/tokens.css?raw";
import { Block, Section, Specimen } from "./frame";

const PARSED = parseTokens(tokensCss);

/** `--name: <raw>` of the `:root` block, to show which tokens are aliases. */
const RAW_ROOT: Record<string, string> = (() => {
  const open = tokensCss.indexOf(":root {");
  const body = tokensCss.slice(open, tokensCss.indexOf("\n}", open));
  const out: Record<string, string> = {};
  for (const m of body.matchAll(/--([\w-]+):\s*([^;]+);/g)) {
    if (m[1] && m[2]) out[m[1]] = m[2].trim();
  }
  return out;
})();

function ratio(fg: string, bg: string, mode: Mode): number | null {
  const f = PARSED[mode][fg];
  const b = PARSED[mode][bg];
  return f && b ? contrastRatio(over(f, b), oklchToSrgb(b)) : null;
}

const fmt = (n: number) => n.toFixed(2);

/** The first contrast pair a token is checked in against `background`. */
const PAIR_ON_BACKGROUND = new Map(
  CONTRAST_PAIRS.filter((p) => p.bg === "background").map((p) => [p.fg, p]),
);

const SWATCH_GROUPS = ["surface", "brand", "semantic", "status", "chart", "sidebar"] as const;

function Swatch({ name, mode }: { name: string; mode: Mode }) {
  const { t } = useTranslation("styleguide");
  const colour = PARSED[mode][name];
  const raw = RAW_ROOT[name] ?? "";
  const alias = /^var\(--([\w-]+)\)$/.exec(raw)?.[1];
  const pair = PAIR_ON_BACKGROUND.get(name);
  const r = pair ? ratio(name, "background", mode) : null;
  return (
    <li className="flex min-w-0 flex-col overflow-hidden rounded-lg border bg-card">
      <span
        aria-hidden="true"
        className="h-14 border-b"
        style={{ backgroundColor: `var(--${name})` }}
      />
      <span className="flex min-w-0 flex-col gap-0.5 p-3">
        <span className="truncate font-mono text-sm text-foreground" title={`--${name}`}>
          --{name}
        </span>
        {colour ? (
          <span className="font-mono text-xs text-muted-foreground tabular-nums">
            L {colour.l} · C {colour.c} · H {colour.h}
            {colour.alpha < 1 ? ` · α ${colour.alpha}` : ""}
          </span>
        ) : null}
        {alias ? (
          <span className="truncate text-xs text-muted-foreground" title={`= --${alias}`}>
            {t("colour.alias", { name: alias })}
          </span>
        ) : null}
        {pair && r !== null ? (
          <span className="mt-1 flex items-center gap-1 text-xs tabular-nums">
            <CheckIcon aria-hidden="true" className="size-3 text-success" />
            <span className="text-muted-foreground">
              {t("colour.onBackground", { ratio: fmt(r), min: pair.min })}
            </span>
          </span>
        ) : null}
      </span>
    </li>
  );
}

export function ColourSection() {
  const { t } = useTranslation("styleguide");
  const { resolvedTheme } = useTheme();
  return (
    <Section id="colour">
      {SWATCH_GROUPS.map((group) => (
        <Block
          key={group}
          title={t(`colour.groups.${group}.title`)}
          description={t(`colour.groups.${group}.body`)}
        >
          <ul className="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-4 xl:grid-cols-5">
            {TOKENS[group].map((name) => (
              <Swatch key={name} name={name} mode={resolvedTheme} />
            ))}
          </ul>
        </Block>
      ))}
      <Block title={t("colour.pairs.title")} description={t("colour.pairs.body")}>
        <Specimen padded={false} className="overflow-x-auto">
          <table className="w-full border-separate border-spacing-0 text-sm">
            <thead>
              <tr className="text-left text-xs tracking-wide text-muted-foreground uppercase">
                <th scope="col" className="px-4 py-3 font-medium">
                  {t("colour.pairs.sample")}
                </th>
                <th scope="col" className="px-4 py-3 font-medium">
                  {t("colour.pairs.foreground")}
                </th>
                <th scope="col" className="px-4 py-3 font-medium">
                  {t("colour.pairs.background")}
                </th>
                <th scope="col" className="px-4 py-3 text-right font-medium">
                  {t("colour.pairs.min")}
                </th>
                <th scope="col" className="px-4 py-3 text-right font-medium">
                  {t("colour.pairs.light")}
                </th>
                <th scope="col" className="px-4 py-3 text-right font-medium">
                  {t("colour.pairs.dark")}
                </th>
              </tr>
            </thead>
            <tbody>
              {CONTRAST_PAIRS.map((p) => (
                <tr key={`${p.fg}/${p.bg}`}>
                  <td className="border-t px-4 py-2">
                    <span
                      className="inline-flex h-7 w-12 items-center justify-center rounded-md border text-sm font-semibold"
                      style={{ backgroundColor: `var(--${p.bg})` }}
                    >
                      {p.min === 4.5 ? (
                        <span style={{ color: `var(--${p.fg})` }}>Aa</span>
                      ) : (
                        // A boundary or a graphic, not text: shown as a stroke.
                        <span
                          aria-hidden="true"
                          className="block h-1 w-7 rounded-full"
                          style={{ backgroundColor: `var(--${p.fg})` }}
                        />
                      )}
                    </span>
                  </td>
                  <td className="border-t px-4 py-2 font-mono text-sm whitespace-nowrap">
                    --{p.fg}
                  </td>
                  <td className="border-t px-4 py-2 font-mono text-sm whitespace-nowrap">
                    --{p.bg}
                  </td>
                  <td className="border-t px-4 py-2 text-right tabular-nums">{p.min}</td>
                  {(["light", "dark"] as const).map((mode) => {
                    const r = ratio(p.fg, p.bg, mode);
                    return (
                      <td
                        key={mode}
                        className={cn(
                          "border-t px-4 py-2 text-right whitespace-nowrap tabular-nums",
                          mode === resolvedTheme
                            ? "font-semibold text-foreground"
                            : "text-muted-foreground",
                        )}
                      >
                        {r === null ? "—" : `${fmt(r)} : 1`}
                      </td>
                    );
                  })}
                </tr>
              ))}
            </tbody>
          </table>
        </Specimen>
      </Block>
    </Section>
  );
}

const SCALE = [
  { token: "3xl", px: "36 / 42", use: "scale.3xl" },
  { token: "2xl", px: "28 / 34", use: "scale.2xl" },
  { token: "xl", px: "22 / 28", use: "scale.xl" },
  { token: "lg", px: "18 / 26", use: "scale.lg" },
  { token: "editor", px: "17 / 28", use: "scale.editor" },
  { token: "md", px: "16 / 24", use: "scale.md" },
  { token: "base", px: "14 / 20", use: "scale.base" },
  { token: "sm", px: "13 / 18", use: "scale.sm" },
  { token: "xs", px: "12 / 16", use: "scale.xs" },
] as const;

const SIZE_CLASS: Record<(typeof SCALE)[number]["token"], string> = {
  "3xl": "text-3xl font-semibold",
  "2xl": "text-2xl font-semibold",
  xl: "text-xl font-semibold",
  lg: "text-lg font-semibold",
  editor: "text-editor",
  md: "text-md",
  base: "text-base",
  sm: "text-sm",
  xs: "text-xs",
};

const INTER_WEIGHTS = [
  { w: 400, key: "regular" },
  { w: 500, key: "medium" },
  { w: 600, key: "semibold" },
  { w: 700, key: "bold" },
] as const;

export function TypeSection() {
  const { t } = useTranslation("styleguide");
  return (
    <Section id="type">
      <Block title={t("type.scale.title")} description={t("type.scale.body")}>
        <Specimen padded={false}>
          <ul className="divide-y">
            {SCALE.map((s) => (
              <li
                key={s.token}
                className="grid grid-cols-1 gap-1 px-5 py-4 sm:grid-cols-[7rem_1fr] sm:items-baseline sm:gap-6"
              >
                <span className="flex items-baseline gap-2 font-mono text-xs text-muted-foreground tabular-nums">
                  <span className="text-foreground">{s.token}</span>
                  {s.px}
                </span>
                <span className="flex min-w-0 flex-col gap-0.5">
                  <span className={cn("text-foreground", SIZE_CLASS[s.token])}>
                    {t("type.sample")}
                  </span>
                  <span className="text-xs text-muted-foreground">{t(`type.${s.use}`)}</span>
                </span>
              </li>
            ))}
          </ul>
        </Specimen>
      </Block>

      <div className="grid gap-6 lg:grid-cols-2">
        <Block title="Inter Variable" description={t("type.inter")}>
          <Specimen className="flex flex-col gap-5">
            <div className="flex items-end justify-between gap-4">
              <span className="text-[4.5rem] leading-none font-semibold tracking-[-0.03em]">
                Aa
              </span>
              <span className="text-right font-mono text-xs text-muted-foreground">
                wght 100–900
                <br />
                opsz 14–32
              </span>
            </div>
            <ul className="flex flex-col gap-2">
              {INTER_WEIGHTS.map(({ w, key }) => (
                <li key={w} className="flex items-baseline justify-between gap-4">
                  <span className="text-lg" style={{ fontWeight: w }}>
                    {t(`type.weights.${key}`)}
                  </span>
                  <span className="font-mono text-xs text-muted-foreground tabular-nums">{w}</span>
                </li>
              ))}
              <li className="flex items-baseline justify-between gap-4">
                <span className="text-lg italic">{t("type.weights.italic")}</span>
                <span className="font-mono text-xs text-muted-foreground">italic</span>
              </li>
            </ul>
            <p className="border-t pt-4 text-sm text-muted-foreground">
              <span className="text-foreground tabular-nums">0123456789</span> · {t("type.tabular")}
            </p>
          </Specimen>
        </Block>
        <Block title="Geist Mono Variable" description={t("type.mono")}>
          <Specimen className="flex flex-col gap-5">
            <div className="flex items-end justify-between gap-4">
              <span className="font-mono text-[4.5rem] leading-none font-medium tracking-[-0.04em]">
                {"{}"}
              </span>
              <span className="text-right font-mono text-xs text-muted-foreground">
                wght 100–900
              </span>
            </div>
            <ul className="flex flex-col gap-2 font-mono text-sm">
              {[400, 500, 600].map((w) => (
                <li key={w} className="flex items-baseline justify-between gap-4">
                  <span style={{ fontWeight: w }}>post_7f3a9c · v12</span>
                  <span className="text-xs text-muted-foreground tabular-nums">{w}</span>
                </li>
              ))}
            </ul>
            <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1.5 border-t pt-4 text-sm">
              <dt className="text-muted-foreground">{t("type.ids.slug")}</dt>
              <dd className="truncate font-mono" title="/blog/hello-world">
                /blog/hello-world
              </dd>
              <dt className="text-muted-foreground">{t("type.ids.version")}</dt>
              <dd className="font-mono tabular-nums">"42"</dd>
              <dt className="text-muted-foreground">{t("type.ids.hash")}</dt>
              <dd className="truncate font-mono" title="sha256:9f86d081884c7d65">
                sha256:9f86d081884c7d65
              </dd>
            </dl>
          </Specimen>
        </Block>
      </div>
    </Section>
  );
}

const SPACING = [1, 2, 3, 4, 6, 8, 10, 12, 16] as const;
const RADII = [
  { name: "sm", cls: "rounded-sm" },
  { name: "md", cls: "rounded-md" },
  { name: "lg", cls: "rounded-lg" },
  { name: "xl", cls: "rounded-xl" },
] as const;
const SHADOWS = [
  { name: "shadow-1", cls: "shadow-1" },
  { name: "shadow-2", cls: "shadow-2" },
  { name: "shadow-3", cls: "shadow-3" },
] as const;

const readVar = (name: string) =>
  getComputedStyle(document.documentElement).getPropertyValue(name).trim();

/** A custom property's computed value on <html>, re-read when its attributes change. */
function useCssVar(name: string): string {
  const [value, setValue] = useState(() => readVar(name));
  useEffect(() => {
    const observer = new MutationObserver(() => setValue(readVar(name)));
    observer.observe(document.documentElement, { attributes: true });
    return () => observer.disconnect();
  }, [name]);
  return value;
}

export function LayoutSection() {
  const { t } = useTranslation("styleguide");
  const { density } = useDensity();
  const { resolvedTheme } = useTheme();
  const controlH = useCssVar("--control-h");
  const rowH = useCssVar("--row-h");
  const pagePad = useCssVar("--page-pad");
  return (
    <Section id="layout">
      <div className="grid gap-6 lg:grid-cols-2">
        <Block title={t("layout.spacing.title")} description={t("layout.spacing.body")}>
          <Specimen className="flex flex-col gap-2.5">
            {SPACING.map((n) => (
              <div key={n} className="grid grid-cols-[3rem_3rem_1fr] items-center gap-3">
                <span className="font-mono text-xs text-foreground tabular-nums">{n}</span>
                <span className="font-mono text-xs text-muted-foreground tabular-nums">
                  {n * 4}px
                </span>
                <span
                  aria-hidden="true"
                  className="h-3 rounded-sm bg-primary/70"
                  style={{ width: `${n * 4}px` }}
                />
              </div>
            ))}
          </Specimen>
        </Block>
        <div className="flex flex-col gap-6">
          <Block title={t("layout.radius.title")} description={t("layout.radius.body")}>
            <Specimen className="grid grid-cols-4 gap-3">
              {RADII.map((r) => (
                <div key={r.name} className="flex flex-col items-center gap-2">
                  <span
                    aria-hidden="true"
                    className={cn("size-14 border-2 border-primary bg-primary/10", r.cls)}
                  />
                  <span className="font-mono text-xs text-muted-foreground">{r.name}</span>
                </div>
              ))}
            </Specimen>
          </Block>
          <Block
            title={t("layout.elevation.title")}
            description={t(
              resolvedTheme === "dark" ? "layout.elevation.dark" : "layout.elevation.light",
            )}
          >
            <div className="grid grid-cols-3 gap-4 rounded-xl bg-muted p-5">
              {SHADOWS.map((s) => (
                <div
                  key={s.name}
                  className={cn(
                    "flex h-20 items-end rounded-lg border bg-card p-3 font-mono text-xs text-muted-foreground",
                    s.cls,
                  )}
                >
                  {s.name}
                </div>
              ))}
            </div>
          </Block>
        </div>
      </div>
      <Block title={t("layout.density.title")} description={t("layout.density.body")}>
        <Specimen className="flex flex-col gap-5">
          <dl className="grid grid-cols-1 gap-3 sm:grid-cols-3">
            {(
              [
                { name: "--control-h", value: controlH, use: "layout.density.control" },
                { name: "--row-h", value: rowH, use: "layout.density.row" },
                { name: "--page-pad", value: pagePad, use: "layout.density.pad" },
              ] as const
            ).map((v) => (
              <div key={v.name} className="flex flex-col gap-0.5 rounded-lg border p-3">
                <dt className="font-mono text-xs text-muted-foreground">{v.name}</dt>
                <dd className="text-xl font-semibold tabular-nums">{v.value}</dd>
                <dd className="text-xs text-muted-foreground">{t(v.use)}</dd>
              </div>
            ))}
          </dl>
          <div className="flex flex-wrap items-center gap-2">
            <Input
              aria-label={t("layout.density.sampleInput")}
              placeholder={t("layout.density.sampleInput")}
              className="max-w-64"
            />
            <Button>{t("layout.density.sampleButton")}</Button>
            <Badge variant="outline" className="font-mono">
              {density}
            </Badge>
          </div>
        </Specimen>
      </Block>
    </Section>
  );
}

const DURATIONS = [
  { name: "--dur-instant", ms: 80, use: "motion.instant" },
  { name: "--dur-fast", ms: 120, use: "motion.fast" },
  { name: "--dur-base", ms: 180, use: "motion.base" },
  { name: "--dur-slow", ms: 240, use: "motion.slow" },
] as const;
const EASINGS = [
  { name: "--ease-out", p: [0.22, 1, 0.36, 1], use: "motion.easeOut" },
  { name: "--ease-in-out", p: [0.65, 0, 0.35, 1], use: "motion.easeInOut" },
] as const;

function usePrefersReducedMotion(): boolean {
  const query = "(prefers-reduced-motion: reduce)";
  const [reduced, setReduced] = useState(
    () => typeof matchMedia === "function" && matchMedia(query).matches,
  );
  useEffect(() => {
    if (typeof matchMedia !== "function") return;
    const mq = matchMedia(query);
    const on = () => setReduced(mq.matches);
    mq.addEventListener("change", on);
    return () => mq.removeEventListener("change", on);
  }, []);
  return reduced;
}

export function MotionSection() {
  const { t } = useTranslation("styleguide");
  const reduced = usePrefersReducedMotion();
  const [played, setPlayed] = useState(false);
  return (
    <Section id="motion">
      <div className="grid gap-6 lg:grid-cols-[3fr_2fr]">
        <Block title={t("motion.durations.title")} description={t("motion.durations.body")}>
          <Specimen className="flex flex-col gap-4">
            {DURATIONS.map((d) => (
              <div key={d.name} className="flex flex-col gap-1.5">
                <div className="flex items-baseline justify-between gap-3">
                  <span className="font-mono text-sm">{d.name}</span>
                  <span className="text-sm text-muted-foreground tabular-nums">
                    {d.ms} ms · {t(d.use)}
                  </span>
                </div>
                <div className="h-2 overflow-hidden rounded-full bg-muted">
                  <span
                    aria-hidden="true"
                    className="block h-full origin-left rounded-full bg-primary transition-transform ease-out"
                    style={{
                      transform: `scaleX(${played ? 1 : d.ms / 240})`,
                      transitionDuration: `var(${d.name})`,
                    }}
                  />
                </div>
              </div>
            ))}
            <div className="flex items-center gap-3 pt-1">
              <Button variant="outline" size="sm" onClick={() => setPlayed((p) => !p)}>
                <PlayIcon data-icon="inline-start" />
                {t("motion.play")}
              </Button>
              <span className="text-xs text-muted-foreground">{t("motion.playHint")}</span>
            </div>
          </Specimen>
        </Block>
        <Block title={t("motion.easing.title")} description={t("motion.easing.body")}>
          <Specimen className="grid grid-cols-2 gap-4">
            {EASINGS.map((e) => {
              const [x1, y1, x2, y2] = e.p;
              return (
                <figure key={e.name} className="flex flex-col gap-2">
                  <svg
                    viewBox="-4 -4 108 108"
                    aria-hidden="true"
                    className="w-full rounded-lg bg-muted"
                  >
                    <path d="M0 100 L100 0" className="stroke-border" strokeWidth="1" fill="none" />
                    <path
                      d={`M0 100 C ${x1 * 100} ${100 - y1 * 100} ${x2 * 100} ${100 - y2 * 100} 100 0`}
                      className="stroke-primary"
                      strokeWidth="2.5"
                      fill="none"
                      strokeLinecap="round"
                    />
                  </svg>
                  <figcaption className="flex flex-col">
                    <span className="font-mono text-xs">{e.name}</span>
                    <span className="text-xs text-muted-foreground">{t(e.use)}</span>
                  </figcaption>
                </figure>
              );
            })}
          </Specimen>
        </Block>
      </div>
      <div
        role="note"
        className="flex flex-col gap-1 rounded-xl border border-info/40 bg-info/5 p-4 sm:flex-row sm:items-center sm:gap-3"
      >
        <Badge variant="outline" className="w-fit border-info/40 text-info">
          {reduced ? t("motion.reduced.on") : t("motion.reduced.off")}
        </Badge>
        <p className="text-sm text-foreground">{t("motion.reduced.body")}</p>
      </div>
    </Section>
  );
}
