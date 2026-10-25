// A minimal ⌘K palette: jump to any screen the nav shows, or open the shortcut sheet.
// Task 11 replaces it with the full command palette (recent items, create, live
// content search, preferences); the shell's trigger and `mod+k` stay.

import { useNavigate } from "@tanstack/react-router";
import { KeyboardIcon } from "lucide-react";
import { useTranslation } from "react-i18next";
import { KbdCombo } from "@/components/kit/kbd-combo";
import {
  Command,
  CommandDialog,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
  CommandShortcut,
} from "@/components/ui/command";
import type { NavSection } from "./nav";

export interface GoToPaletteProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  sections: NavSection[];
  onShortcuts: () => void;
}

export function GoToPalette({ open, onOpenChange, sections, onShortcuts }: GoToPaletteProps) {
  const { t } = useTranslation("shell");
  const navigate = useNavigate();
  const items = sections.flatMap((s) => s.items);
  return (
    <CommandDialog
      open={open}
      onOpenChange={onOpenChange}
      title={t("palette.title")}
      description={t("palette.description")}
    >
      <Command label={t("palette.title")}>
        <CommandInput placeholder={t("palette.placeholder")} />
        <CommandList>
          <CommandEmpty>{t("palette.empty")}</CommandEmpty>
          <CommandGroup heading={t("palette.goTo")}>
            {items.map((item) => (
              <CommandItem
                key={item.id}
                value={`${item.label} ${item.id}`}
                onSelect={() => {
                  onOpenChange(false);
                  void navigate({ to: item.to as "/" });
                }}
              >
                <item.icon />
                {item.label}
                {item.shortcut ? (
                  <CommandShortcut>
                    <KbdCombo keys={item.shortcut} />
                  </CommandShortcut>
                ) : null}
              </CommandItem>
            ))}
            <CommandItem
              value={`${t("shortcuts.title")} shortcuts`}
              onSelect={() => {
                onOpenChange(false);
                onShortcuts();
              }}
            >
              <KeyboardIcon />
              {t("shortcuts.title")}
              <CommandShortcut>
                <KbdCombo keys="?" />
              </CommandShortcut>
            </CommandItem>
          </CommandGroup>
        </CommandList>
      </Command>
    </CommandDialog>
  );
}
