// A plugin's settings (`GET/PATCH /plugins/{name}/settings`; 05 §4c, 07 §1 H-P4) as a
// `SettingsForm`: only the changed keys are sent, `null` resets one. Secrets are
// write-only: the form never holds a stored value, an untouched secret is left out of the
// `PATCH` (kept), a typed one sets it and Remove sends `null`. The `PATCH` is a plain
// awaited call (never a TanStack mutation, whose cache would keep the secret as its
// variables); afterwards the settings are read again so a secret shows "Set". Keys whose
// stored value no longer fits after an update (`invalid`) are flagged. Leaving the page
// with unsaved changes asks first.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { INLINE_ERRORS } from "@/api/client";
import { qk } from "@/api/keys";
import { getPluginSettings, patchPluginSettings, pluginSettingsQuery } from "@/api/plugins";
import { Alert } from "@/auth/auth-layout";
import { ErrorState } from "@/components/kit/error-state";
import { SettingsForm } from "@/components/kit/settings-form";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { useLeaveGuard } from "@/editor/leave-guard";
import { DESTRUCTIVE } from "@/lib/destructive";

export function PluginSettings({ name }: { name: string }) {
  const { t } = useTranslation("admin");
  const queryClient = useQueryClient();
  const settings = useQuery({ ...pluginSettingsQuery(name), meta: INLINE_ERRORS });
  const [dirty, setDirty] = useState(false);
  const [saving, setSaving] = useState(false);
  const leave = useLeaveGuard(dirty && !saving);

  let body;
  if (settings.data) {
    const data = settings.data;
    body =
      data.schema.length === 0 ? (
        <p className="text-sm text-muted-foreground">{t("plugins.settings.none")}</p>
      ) : (
        <div className="flex flex-col gap-4">
          {data.invalid.length > 0 ? (
            <div data-slot="settings-invalid">
              <Alert tone="warning">
                {t("plugins.settings.invalid", { count: data.invalid.length })}
              </Alert>
            </div>
          ) : null}
          <SettingsForm
            schema={data.schema}
            values={data.values}
            secrets={data.secrets}
            invalid={data.invalid}
            staleKey={t("plugins.settings.staleKey", { name })}
            reload={async () => (await getPluginSettings(name)).values}
            onDirtyChange={setDirty}
            onSubmit={async (patch) => {
              setSaving(true);
              try {
                const saved = await patchPluginSettings(name, patch);
                // The answer holds no secret value, only whether each is set.
                queryClient.setQueryData(qk.pluginSettings(name), saved);
                void queryClient.invalidateQueries({ queryKey: qk.pluginSettings(name) });
                return saved.values;
              } finally {
                setSaving(false);
              }
            }}
          />
        </div>
      );
  } else if (settings.isError) {
    body = <ErrorState error={settings.error} retry={() => void settings.refetch()} />;
  } else {
    body = (
      <div
        aria-busy="true"
        aria-label={t("plugins.settings.loading")}
        className="flex flex-col gap-4"
      >
        <Skeleton className="h-9 w-full" />
        <Skeleton className="h-9 w-full" />
        <Skeleton className="h-9 w-2/3" />
      </div>
    );
  }

  return (
    <>
      {body}
      <AlertDialog open={leave.blocked} onOpenChange={(open) => (open ? undefined : leave.stay())}>
        <AlertDialogContent data-slot="plugin-settings-leave">
          <AlertDialogHeader>
            <AlertDialogTitle>{t("settings.leave.title")}</AlertDialogTitle>
            <AlertDialogDescription>{t("settings.leave.body")}</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t("settings.leave.stay")}</AlertDialogCancel>
            <Button variant="destructive" className={DESTRUCTIVE} onClick={leave.leave}>
              {t("settings.leave.leave")}
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}
