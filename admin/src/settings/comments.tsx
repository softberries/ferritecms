// Comments: whether posts take comments, how guest comments are moderated (`all` holds
// every one; `known` lets a commenter with an approved comment post directly, which
// anyone who knows their exact name and e-mail can use), and threads per public page.

import { useTranslation } from "react-i18next";
import type { SettingsDto } from "@/api/settings";
import { FormField, SwitchRow } from "@/components/kit/form-field";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { ChoiceCards } from "./choice-cards";
import { DEFAULTS, LIMITS, type Moderation, boolSpec, intSpec, moderationSpec } from "./fields";
import { GroupFormFrame } from "./group-frame";
import { useGroupForm } from "./use-group-form";

const SPECS = {
  "comments.enabled": boolSpec(DEFAULTS.commentsEnabled),
  "comments.moderation": moderationSpec,
  "comments.per_page": intSpec(LIMITS.perPage.min, LIMITS.perPage.max),
};

export function CommentsGroup({ settings }: { settings: SettingsDto }) {
  const { t } = useTranslation("admin");
  const form = useGroupForm(SPECS, settings, {
    resource: t("settings.resource", { group: t("settings.groups.comments.title") }),
  });
  const off = form.saving;
  return (
    <GroupFormFrame
      title={t("settings.groups.comments.title")}
      description={t("settings.groups.comments.description")}
      form={form}
    >
      <SwitchRow label={t("settings.comments.enabled")} hint={t("settings.comments.enabledHint")}>
        {(control) => (
          <Switch
            {...control}
            checked={form.draft["comments.enabled"]}
            disabled={off}
            onCheckedChange={(on) => form.set("comments.enabled", on)}
          />
        )}
      </SwitchRow>
      <ChoiceCards<Moderation>
        label={t("settings.comments.moderation")}
        slot="moderation-choice"
        value={form.draft["comments.moderation"]}
        disabled={off}
        onChange={(v) => form.set("comments.moderation", v)}
        choices={[
          {
            value: "known",
            title: t("settings.comments.known"),
            description: t("settings.comments.knownHint"),
          },
          {
            value: "all",
            title: t("settings.comments.all"),
            description: t("settings.comments.allHint"),
          },
        ]}
      />
      <FormField
        slot="setting-comments-per-page"
        label={t("settings.comments.perPage")}
        hint={t("settings.comments.perPageHint", { value: DEFAULTS.commentsPerPage })}
        error={form.errors["comments.per_page"]}
      >
        {(control) => (
          <Input
            {...control}
            inputMode="numeric"
            autoComplete="off"
            placeholder={String(DEFAULTS.commentsPerPage)}
            value={form.draft["comments.per_page"]}
            disabled={off}
            className="max-w-28 font-mono tabular-nums"
            onChange={(e) => form.set("comments.per_page", e.currentTarget.value)}
          />
        )}
      </FormField>
    </GroupFormFrame>
  );
}
