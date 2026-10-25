// The shell's top bar (56 px, sticky; the shell sets `--sticky-top` to its height):
// the sidebar toggle, breadcrumbs from the matched routes' `staticData.crumb`, the
// "Search engines discouraged" pill, the ⌘K trigger, the "New" split button (only with
// `edit_own`) and the account menu.

import { Link, useMatches } from "@tanstack/react-router";
import {
  ChevronDownIcon,
  EyeOffIcon,
  FilePlusIcon,
  FilesIcon,
  PlusIcon,
  SearchIcon,
} from "lucide-react";
import { Fragment } from "react";
import { useTranslation } from "react-i18next";
import type { MeDto } from "@/auth/session";
import { KbdCombo } from "@/components/kit/kbd-combo";
import {
  Breadcrumb,
  BreadcrumbItem,
  BreadcrumbLink,
  BreadcrumbList,
  BreadcrumbPage,
  BreadcrumbSeparator,
} from "@/components/ui/breadcrumb";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { SidebarTrigger } from "@/components/ui/sidebar";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { UserMenu } from "./user-menu";

export interface TopBarProps {
  me: MeDto;
  discouraged: boolean;
  onPalette: () => void;
  onShortcuts: () => void;
  onSignOut: () => void;
}

interface Crumb {
  id: string;
  label: string;
  /** The admin path of the crumb (not the last). */
  to: string;
}

function useCrumbs(): Crumb[] {
  const { t } = useTranslation("shell");
  return useMatches({
    select: (matches) => {
      const crumbs: Crumb[] = [];
      for (const m of matches) {
        const crumb = m.staticData.crumb;
        if (crumb) crumbs.push({ id: m.id, label: crumb(t), to: m.pathname });
      }
      if (matches.some((m) => m.status === "notFound")) {
        crumbs.push({ id: "not-found", label: t("crumbs.notFound"), to: "" });
      }
      return crumbs;
    },
  });
}

function Crumbs() {
  const crumbs = useCrumbs();
  return (
    <Breadcrumb className="min-w-0 flex-1">
      <BreadcrumbList className="flex-nowrap">
        {crumbs.map((c, i) => {
          const last = i === crumbs.length - 1;
          return (
            <Fragment key={c.id}>
              {i > 0 ? <BreadcrumbSeparator className="max-sm:hidden" /> : null}
              <BreadcrumbItem className={last ? "min-w-0" : "max-sm:hidden"}>
                {last ? (
                  <BreadcrumbPage className="truncate font-medium" title={c.label}>
                    {c.label}
                  </BreadcrumbPage>
                ) : (
                  <BreadcrumbLink asChild>
                    <Link to={c.to as "/"}>{c.label}</Link>
                  </BreadcrumbLink>
                )}
              </BreadcrumbItem>
            </Fragment>
          );
        })}
      </BreadcrumbList>
    </Breadcrumb>
  );
}

function DiscouragedPill() {
  const { t } = useTranslation("shell");
  return (
    <span
      data-slot="discouraged-pill"
      title={t("topBar.discouragedHint")}
      className="inline-flex h-7 shrink-0 items-center gap-1.5 rounded-full border border-warning/45 bg-warning/10 px-2.5 text-xs font-medium text-foreground"
    >
      <EyeOffIcon aria-hidden="true" className="size-3.5 text-warning" />
      <span className="max-lg:sr-only">{t("topBar.discouraged")}</span>
    </span>
  );
}

function PaletteTrigger({ onPalette }: { onPalette: () => void }) {
  const { t } = useTranslation("shell");
  return (
    <>
      <Button
        variant="outline"
        onClick={onPalette}
        className="w-60 justify-start gap-2 bg-muted/40 px-2.5 font-normal text-muted-foreground shadow-none hover:text-foreground max-md:hidden lg:w-72"
      >
        <SearchIcon data-icon="inline-start" />
        <span className="flex-1 truncate text-left">{t("topBar.search")}</span>
        <KbdCombo keys="mod+k" />
      </Button>
      <Button
        variant="ghost"
        size="icon"
        onClick={onPalette}
        aria-label={t("topBar.searchShort")}
        className="md:hidden"
      >
        <SearchIcon />
      </Button>
    </>
  );
}

function NewButton() {
  const { t } = useTranslation("shell");
  return (
    <div className="flex shrink-0 items-stretch" data-slot="new-split">
      <Button asChild className="rounded-r-none pr-2.5 max-sm:px-2.5">
        <Link to={"/content/post/new" as "/"} aria-label={t("topBar.newPost")}>
          <PlusIcon data-icon="inline-start" />
          <span className="max-sm:hidden">{t("topBar.new")}</span>
        </Link>
      </Button>
      <DropdownMenu modal={false}>
        <DropdownMenuTrigger asChild>
          <Button
            size="icon"
            className="w-7 rounded-l-none border-l border-primary-foreground/25 pointer-coarse:w-9"
            aria-label={t("topBar.newMore")}
          >
            <ChevronDownIcon />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" sideOffset={8} className="w-48">
          <DropdownMenuItem asChild>
            <Link to={"/content/post/new" as "/"}>
              <FilePlusIcon />
              {t("topBar.newPost")}
            </Link>
          </DropdownMenuItem>
          <DropdownMenuItem asChild>
            <Link to={"/content/page/new" as "/"}>
              <FilesIcon />
              {t("topBar.newPage")}
            </Link>
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  );
}

export function TopBar({ me, discouraged, onPalette, onShortcuts, onSignOut }: TopBarProps) {
  const { t } = useTranslation();
  return (
    <header
      data-slot="top-bar"
      className="sticky top-0 z-20 flex h-14 shrink-0 items-center gap-2 border-b bg-background/85 px-3 backdrop-blur-md supports-backdrop-filter:bg-background/70 sm:gap-3 sm:px-4"
    >
      <Tooltip>
        <TooltipTrigger asChild>
          <SidebarTrigger className="-ml-1 size-control text-muted-foreground hover:text-foreground" />
        </TooltipTrigger>
        <TooltipContent side="bottom" className="flex items-center gap-2">
          {t("ui.toggleSidebar")}
          <KbdCombo keys="mod+b" />
        </TooltipContent>
      </Tooltip>
      <span aria-hidden="true" className="h-5 w-px shrink-0 bg-border max-sm:hidden" />
      <Crumbs />
      <div className="flex shrink-0 items-center gap-1.5 sm:gap-2">
        {discouraged ? <DiscouragedPill /> : null}
        <PaletteTrigger onPalette={onPalette} />
        {me.capabilities.includes("edit_own") ? <NewButton /> : null}
        <UserMenu me={me} onShortcuts={onShortcuts} onSignOut={onSignOut} />
      </div>
    </header>
  );
}
