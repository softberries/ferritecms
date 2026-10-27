// The account menu at the right of the top bar: who is signed in (display name, role),
// a link to the Account screen, the theme (light / dark / system) and density, the
// shortcut sheet, and sign out.
// Non-modal (11 §7: a modal menu fails axe `aria-hidden-focus`).

import { Link } from "@tanstack/react-router";
import {
  KeyboardIcon,
  LogOutIcon,
  MonitorIcon,
  MoonIcon,
  Rows3Icon,
  Rows4Icon,
  SunIcon,
  UserRoundIcon,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import type { MeDto } from "@/auth/session";
import { KbdCombo } from "@/components/kit/kbd-combo";
import { Avatar, AvatarFallback } from "@/components/ui/avatar";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuShortcut,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { type Density, useDensity } from "@/design/density";
import { type ThemePreference, useTheme } from "@/design/theme";

const THEMES = [
  ["light", SunIcon],
  ["dark", MoonIcon],
  ["system", MonitorIcon],
] as const;
const DENSITIES = [
  ["comfortable", Rows3Icon],
  ["compact", Rows4Icon],
] as const;
const ROLES = ["admin", "editor", "author", "contributor"] as const;
type Role = (typeof ROLES)[number];
const isRole = (r: string): r is Role => (ROLES as readonly string[]).includes(r);

/** Up to two initials of the display name (the login when it has none). */
function initials(name: string): string {
  const words = name.trim().split(/\s+/).filter(Boolean);
  const first = words[0]?.[0] ?? "";
  const last = words.length > 1 ? (words[words.length - 1]?.[0] ?? "") : (words[0]?.[1] ?? "");
  return `${first}${last}`.toUpperCase();
}

export interface UserMenuProps {
  me: MeDto;
  onShortcuts: () => void;
  onSignOut: () => void;
}

export function UserMenu({ me, onShortcuts, onSignOut }: UserMenuProps) {
  const { t } = useTranslation("shell");
  const { theme, setTheme } = useTheme();
  const { density, setDensity } = useDensity();
  const name = me.display_name || me.login;
  const role = isRole(me.role) ? t(`user.role.${me.role}`) : me.role;

  return (
    <DropdownMenu modal={false}>
      <DropdownMenuTrigger asChild>
        <Button
          variant="ghost"
          size="icon"
          className="rounded-full"
          aria-label={t("user.menu")}
          title={name}
        >
          <Avatar className="size-7">
            <AvatarFallback className="bg-primary/12 text-xs font-semibold text-foreground">
              {initials(name)}
            </AvatarFallback>
          </Avatar>
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" sideOffset={8} className="w-64">
        <DropdownMenuLabel className="flex flex-col gap-0.5 py-2 font-normal">
          <span className="truncate text-sm font-semibold text-foreground" title={name}>
            {name}
          </span>
          <span className="truncate text-xs text-muted-foreground" title={me.email}>
            {role} · {me.email}
          </span>
        </DropdownMenuLabel>
        <DropdownMenuItem asChild>
          <Link to="/account">
            <UserRoundIcon />
            {t("user.account")}
          </Link>
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuGroup>
          <DropdownMenuLabel className="text-xs text-muted-foreground">
            {t("user.theme")}
          </DropdownMenuLabel>
          <DropdownMenuRadioGroup
            value={theme}
            onValueChange={(v) => setTheme(v as ThemePreference)}
          >
            {THEMES.map(([value, Icon]) => (
              <DropdownMenuRadioItem key={value} value={value} onSelect={(e) => e.preventDefault()}>
                <Icon />
                {t(`theme.${value}`)}
              </DropdownMenuRadioItem>
            ))}
          </DropdownMenuRadioGroup>
        </DropdownMenuGroup>
        <DropdownMenuSeparator />
        <DropdownMenuGroup>
          <DropdownMenuLabel className="text-xs text-muted-foreground">
            {t("user.density")}
          </DropdownMenuLabel>
          <DropdownMenuRadioGroup value={density} onValueChange={(v) => setDensity(v as Density)}>
            {DENSITIES.map(([value, Icon]) => (
              <DropdownMenuRadioItem key={value} value={value} onSelect={(e) => e.preventDefault()}>
                <Icon />
                {t(`density.${value}`)}
              </DropdownMenuRadioItem>
            ))}
          </DropdownMenuRadioGroup>
        </DropdownMenuGroup>
        <DropdownMenuSeparator />
        <DropdownMenuItem onSelect={onShortcuts}>
          <KeyboardIcon />
          {t("user.shortcuts")}
          <DropdownMenuShortcut>
            <KbdCombo keys="?" />
          </DropdownMenuShortcut>
        </DropdownMenuItem>
        <DropdownMenuItem onSelect={onSignOut}>
          <LogOutIcon />
          {t("user.signOut")}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
