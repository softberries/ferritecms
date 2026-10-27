// Customize a theme: its `theme.toml [settings]` as a form (`SettingsForm`) in a sheet,
// over `GET/PATCH /themes/{name}/settings`. Only the changed keys are sent; a reset sends
// `null`. Settings are last-write-wins, so the form asks before overwriting a change made
// since it opened (P31). Settings belong to the theme name (H-T3): a replaced theme keeps
// the values stored under its name.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { INLINE_ERRORS } from "@/api/client";
import { qk } from "@/api/keys";
import { getThemeSettings, patchThemeSettings, themeSettingsQuery } from "@/api/themes";
import { ErrorState } from "@/components/kit/error-state";
import { SettingsForm } from "@/components/kit/settings-form";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";
import { Skeleton } from "@/components/ui/skeleton";

export interface ThemeSettingsSheetProps {
  name: string;
  onClose: () => void;
}

export function ThemeSettingsSheet({ name, onClose }: ThemeSettingsSheetProps) {
  const { t } = useTranslation("admin");
  const queryClient = useQueryClient();
  // Its error shows in the sheet (no toast on top).
  const settings = useQuery({ ...themeSettingsQuery(name), meta: INLINE_ERRORS });
  return (
    <Sheet open onOpenChange={(open) => !open && onClose()}>
      <SheetContent
        side="right"
        data-slot="theme-settings"
        className="w-full gap-0 overflow-y-auto p-0 data-[side=right]:w-full data-[side=right]:sm:max-w-lg"
      >
        <SheetHeader className="border-b p-5 pr-12">
          <SheetTitle className="break-words">{t("themes.settings.title", { name })}</SheetTitle>
          <SheetDescription>{t("themes.settings.description")}</SheetDescription>
        </SheetHeader>
        <div className="p-5">
          {settings.data ? (
            <SettingsForm
              schema={settings.data.schema}
              values={settings.data.values}
              staleKey={t("themes.settings.staleKey", { name })}
              reload={async () => (await getThemeSettings(name)).values}
              onSubmit={async (patch) => {
                const saved = await patchThemeSettings(name, patch);
                queryClient.setQueryData(qk.themeSettings(name), saved);
                return saved.values;
              }}
            />
          ) : settings.isError ? (
            <ErrorState error={settings.error} retry={() => void settings.refetch()} />
          ) : (
            <div
              aria-busy="true"
              aria-label={t("themes.settings.loading")}
              className="flex flex-col gap-4"
            >
              <Skeleton className="h-9 w-full" />
              <Skeleton className="h-9 w-full" />
              <Skeleton className="h-9 w-2/3" />
            </div>
          )}
        </div>
      </SheetContent>
    </Sheet>
  );
}
