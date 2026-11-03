// Content health (M6-4, design 16 §6.3, §6.5): whether the daily run checks external
// links (off by default: the server then contacts every linked site, which learns that
// this site links to it) and after how many days without an edit an item is "stale".

import { TriangleAlertIcon } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { SettingsDto } from "@/api/settings";
import { FormField, SwitchRow } from "@/components/kit/form-field";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { boolSpec, intSpec } from "./fields";
import { GroupFormFrame } from "./group-frame";
import { useGroupForm } from "./use-group-form";

/** `HEALTH_*_DEFAULT` and the bounds in `ferrite-core/src/settings.rs`. */
export const HEALTH_STALE_DEFAULT = 365;
export const HEALTH_STALE_MIN = 30;
export const HEALTH_STALE_MAX = 3650;

const SPECS = {
  "health.external_links": boolSpec(false),
  "health.stale_days": intSpec(HEALTH_STALE_MIN, HEALTH_STALE_MAX),
};

export function HealthGroup({ settings }: { settings: SettingsDto }) {
  const { t } = useTranslation("admin");
  const form = useGroupForm(SPECS, settings, {
    resource: t("settings.resource", { group: t("settings.groups.health.title") }),
  });
  const off = form.saving;
  return (
    <GroupFormFrame
      title={t("settings.groups.health.title")}
      description={t("settings.groups.health.description")}
      form={form}
    >
      <SwitchRow label={t("settings.health.external")} hint={t("settings.health.externalHint")}>
        {(control) => (
          <Switch
            {...control}
            data-slot="health-external"
            checked={form.draft["health.external_links"]}
            disabled={off}
            onCheckedChange={(on) => form.set("health.external_links", on)}
          />
        )}
      </SwitchRow>
      <p className="flex items-start gap-2 text-sm text-muted-foreground">
        <TriangleAlertIcon aria-hidden="true" className="mt-0.5 size-4 shrink-0 text-warning" />
        <span className="text-pretty">{t("settings.health.privacy")}</span>
      </p>
      <FormField
        slot="setting-health-stale"
        label={t("settings.health.stale")}
        hint={t("settings.health.staleHint", {
          value: HEALTH_STALE_DEFAULT,
          min: HEALTH_STALE_MIN,
          max: HEALTH_STALE_MAX,
        })}
        error={form.errors["health.stale_days"]}
      >
        {(control) => (
          <Input
            {...control}
            inputMode="numeric"
            autoComplete="off"
            placeholder={String(HEALTH_STALE_DEFAULT)}
            value={form.draft["health.stale_days"]}
            disabled={off}
            className="max-w-28 font-mono tabular-nums"
            onChange={(e) => form.set("health.stale_days", e.currentTarget.value)}
          />
        )}
      </FormField>
    </GroupFormFrame>
  );
}
