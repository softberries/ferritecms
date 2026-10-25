// The living styleguide at /admin/_styleguide (M2b P12): every token and component as
// shipped. A lazy chunk (autoCodeSplitting), linked from nowhere (no nav, no palette
// entry). It sits under `_app`, whose login guard arrives in Task 9 (pre-flight F19).
// `?section=<id>` shows one section; without it, the whole page.

import { Link, createFileRoute } from "@tanstack/react-router";
import { MonitorIcon, MoonIcon, Rows3Icon, Rows4Icon, SunIcon } from "lucide-react";
import { cn } from "cn";
import { useTranslation } from "react-i18next";
import { Toaster } from "@/components/ui/sonner";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { TooltipProvider } from "@/components/ui/tooltip";
import { useDensity, type Density } from "@/design/density";
import { useTheme, type ThemePreference } from "@/design/theme";
import {
  ControlsSection,
  DisplaySection,
  NavigationSection,
  OverlaysSection,
} from "./-styleguide/components";
import {
  ColourSection,
  LayoutSection,
  MotionSection,
  TypeSection,
} from "./-styleguide/foundations";
import { ChartsSection, DataSection, PatternsSection } from "./-styleguide/patterns";
import { SECTIONS, isSection, type SectionId } from "./-styleguide/sections";

interface StyleguideSearch {
  section?: SectionId;
}

export const Route = createFileRoute("/_app/_styleguide")({
  validateSearch: (search: Record<string, unknown>): StyleguideSearch =>
    isSection(search.section) ? { section: search.section } : {},
  component: StyleguidePage,
});

const RENDER: Record<SectionId, () => React.JSX.Element> = {
  colour: ColourSection,
  type: TypeSection,
  layout: LayoutSection,
  motion: MotionSection,
  controls: ControlsSection,
  display: DisplaySection,
  navigation: NavigationSection,
  overlays: OverlaysSection,
  patterns: PatternsSection,
  data: DataSection,
  charts: ChartsSection,
};

function StyleguidePage() {
  const { t } = useTranslation("styleguide");
  const { section } = Route.useSearch();
  const shown = section ? [section] : SECTIONS;
  return (
    <TooltipProvider>
      <title>{t("pageTitle")}</title>
      {/* `--sticky-top`: the height of this page's own sticky header (11 §7). */}
      <div className="min-h-svh bg-background [--sticky-top:6.5rem] sm:[--sticky-top:4rem]">
        <Header />
        <div className="mx-auto w-full max-w-7xl px-page pb-24 lg:grid lg:grid-cols-[12rem_minmax(0,1fr)] lg:gap-12">
          <SectionNav current={section} />
          <main id="main" className="flex min-w-0 flex-col gap-14 pt-8">
            {shown.map((id) => {
              const Render = RENDER[id];
              return <Render key={id} />;
            })}
          </main>
        </div>
      </div>
      <Toaster position="bottom-right" />
    </TooltipProvider>
  );
}

function Header() {
  const { t } = useTranslation("styleguide");
  const { theme, setTheme } = useTheme();
  const { density, setDensity } = useDensity();
  return (
    <header className="sticky top-0 z-30 border-b bg-background/85 backdrop-blur-md">
      <div className="mx-auto flex w-full max-w-7xl flex-wrap items-center justify-between gap-x-6 gap-y-3 px-page py-3">
        <div className="flex min-w-0 items-center gap-3">
          <span
            aria-hidden="true"
            className="flex size-8 shrink-0 items-center justify-center rounded-lg bg-primary font-mono text-sm font-bold text-primary-foreground"
          >
            Fe
          </span>
          <div className="flex min-w-0 flex-col">
            <h1 className="text-lg leading-tight">{t("title")}</h1>
            <p className="truncate text-xs text-muted-foreground" title={t("subtitle")}>
              {t("subtitle")}
            </p>
          </div>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <ToggleGroup
            type="single"
            variant="outline"
            size="sm"
            value={theme}
            onValueChange={(v) => {
              if (v) setTheme(v as ThemePreference);
            }}
            aria-label={t("controls.theme")}
          >
            {(
              [
                ["light", SunIcon],
                ["dark", MoonIcon],
                ["system", MonitorIcon],
              ] as const
            ).map(([value, Icon]) => (
              <ToggleGroupItem key={value} value={value} aria-label={t(`theme.${value}`)}>
                <Icon />
                <span className="max-sm:hidden">{t(`theme.${value}`)}</span>
              </ToggleGroupItem>
            ))}
          </ToggleGroup>
          <ToggleGroup
            type="single"
            variant="outline"
            size="sm"
            value={density}
            onValueChange={(v) => {
              if (v) setDensity(v as Density);
            }}
            aria-label={t("controls.density")}
          >
            {(
              [
                ["comfortable", Rows3Icon],
                ["compact", Rows4Icon],
              ] as const
            ).map(([value, Icon]) => (
              <ToggleGroupItem key={value} value={value} aria-label={t(`density.${value}`)}>
                <Icon />
                <span className="max-sm:hidden">{t(`density.${value}`)}</span>
              </ToggleGroupItem>
            ))}
          </ToggleGroup>
        </div>
      </div>
    </header>
  );
}

function SectionNav({ current }: { current: SectionId | undefined }) {
  const { t } = useTranslation("styleguide");
  const item = (active: boolean) =>
    cn(
      "flex shrink-0 items-center gap-3 rounded-md px-2.5 py-1.5 text-sm whitespace-nowrap transition-colors",
      active
        ? "bg-muted font-medium text-foreground"
        : "text-muted-foreground hover:bg-muted/60 hover:text-foreground",
    );
  return (
    <nav aria-label={t("nav.label")} className="lg:pt-8">
      <ul className="-mx-page flex gap-1 overflow-x-auto border-b px-page py-2 lg:sticky lg:top-24 lg:mx-0 lg:flex-col lg:overflow-visible lg:border-b-0 lg:px-0 lg:py-0">
        <li>
          <Link
            to="/_styleguide"
            search={{}}
            aria-current={current === undefined ? "page" : undefined}
            className={item(current === undefined)}
          >
            <span aria-hidden="true" className="w-5 font-mono text-xs tabular-nums max-lg:hidden">
              00
            </span>
            {t("nav.all")}
          </Link>
        </li>
        {SECTIONS.map((id, i) => (
          <li key={id}>
            <Link
              to="/_styleguide"
              search={{ section: id }}
              aria-current={current === id ? "page" : undefined}
              className={item(current === id)}
            >
              <span aria-hidden="true" className="w-5 font-mono text-xs tabular-nums max-lg:hidden">
                {String(i + 1).padStart(2, "0")}
              </span>
              {t(`sections.${id}.title`)}
            </Link>
          </li>
        ))}
      </ul>
    </nav>
  );
}
