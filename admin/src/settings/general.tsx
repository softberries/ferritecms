// General: the site's title, tagline and locale (`site.*`). Empty means the value from
// `ferrite.toml`; core's limits (200 and 1000 characters, a 1..=35 character tag) are
// checked before the save.

import { useTranslation } from "react-i18next";
import type { SettingsDto } from "@/api/settings";
import { FormField } from "@/components/kit/form-field";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "cn";
import { LIMITS, charCount, localeSpec, textSpec } from "./fields";
import { GroupFormFrame } from "./group-frame";
import { useGroupForm } from "./use-group-form";

const SPECS = {
  "site.title": textSpec({ max: LIMITS.title, multiline: false }),
  "site.description": textSpec({ max: LIMITS.description, multiline: true }),
  "site.locale": localeSpec,
};

/** A field's hint with "n / max" at its end; the count is tinted once over the limit. */
function CountedHint({ hint, value, max }: { hint: string; value: string; max: number }) {
  const n = charCount(value);
  return (
    <span className="flex items-start justify-between gap-4">
      <span>{hint}</span>
      <span
        aria-hidden="true"
        className={cn("shrink-0 tabular-nums", n > max ? "text-destructive" : undefined)}
      >
        {n} / {max}
      </span>
    </span>
  );
}

export function GeneralGroup({ settings }: { settings: SettingsDto }) {
  const { t } = useTranslation("admin");
  const form = useGroupForm(SPECS, settings, {
    resource: t("settings.resource", { group: t("settings.groups.general.title") }),
  });
  const off = form.saving;
  return (
    <GroupFormFrame
      title={t("settings.groups.general.title")}
      description={t("settings.groups.general.description")}
      form={form}
    >
      <FormField
        slot="setting-site-title"
        label={t("settings.general.title")}
        hint={
          <CountedHint
            hint={t("settings.general.titleHint")}
            value={form.draft["site.title"].trim()}
            max={LIMITS.title}
          />
        }
        error={form.errors["site.title"]}
      >
        {(control) => (
          <Input
            {...control}
            value={form.draft["site.title"]}
            disabled={off}
            autoComplete="off"
            onChange={(e) => form.set("site.title", e.currentTarget.value)}
          />
        )}
      </FormField>
      <FormField
        slot="setting-site-description"
        label={t("settings.general.tagline")}
        hint={
          <CountedHint
            hint={t("settings.general.taglineHint")}
            value={form.draft["site.description"]}
            max={LIMITS.description}
          />
        }
        error={form.errors["site.description"]}
      >
        {(control) => (
          <Textarea
            {...control}
            rows={3}
            value={form.draft["site.description"]}
            disabled={off}
            onChange={(e) => form.set("site.description", e.currentTarget.value)}
          />
        )}
      </FormField>
      <FormField
        slot="setting-site-locale"
        label={t("settings.general.locale")}
        hint={t("settings.general.localeHint")}
        error={form.errors["site.locale"]}
      >
        {(control) => (
          <Input
            {...control}
            value={form.draft["site.locale"]}
            disabled={off}
            spellCheck={false}
            autoComplete="off"
            className="max-w-48 font-mono"
            onChange={(e) => form.set("site.locale", e.currentTarget.value)}
          />
        )}
      </FormField>
    </GroupFormFrame>
  );
}
