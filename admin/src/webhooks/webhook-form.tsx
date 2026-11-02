// The fields a webhook is made of (the New webhook dialog and the detail's settings):
// name, URL, events and, when a content event is chosen, the content-type filter. Field
// errors come from the form's own checks or the API's `422` (`errors[0].field`).

import { useQuery } from "@tanstack/react-query";
import { useId } from "react";
import { useTranslation } from "react-i18next";
import { typesQuery } from "@/api/types";
import { eventCatalogueQuery } from "@/api/webhooks";
import { FormField } from "@/components/kit/form-field";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { EventPicker } from "./event-picker";
import { type DraftErrors, type WebhookDraft, hasContentEvents } from "./webhooks-model";

export function WebhookFields({
  draft,
  onChange,
  errors,
  disabled,
}: {
  draft: WebhookDraft;
  onChange: (next: WebhookDraft, field: keyof DraftErrors) => void;
  errors: DraftErrors;
  disabled?: boolean;
}) {
  const { t } = useTranslation("webhooks");
  const catalogue = useQuery(eventCatalogueQuery);
  const types = useQuery(typesQuery);
  const typesLabel = useId();
  const contentTypes = (types.data ?? []).filter((ty) => ty.name !== "media");
  return (
    <div className="flex min-w-0 flex-col gap-5">
      <FormField label={t("form.name")} hint={t("form.nameHint")} error={errors.name}>
        {(control) => (
          <Input
            {...control}
            value={draft.name}
            autoComplete="off"
            maxLength={120}
            disabled={disabled}
            onChange={(e) => onChange({ ...draft, name: e.currentTarget.value }, "name")}
          />
        )}
      </FormField>
      <FormField label={t("form.url")} hint={t("form.urlHint")} error={errors.url}>
        {(control) => (
          <Input
            {...control}
            type="url"
            inputMode="url"
            value={draft.url}
            autoComplete="off"
            spellCheck={false}
            placeholder="https://"
            disabled={disabled}
            className="font-mono text-[0.8125rem]"
            onChange={(e) => onChange({ ...draft, url: e.currentTarget.value }, "url")}
          />
        )}
      </FormField>
      <EventPicker
        types={catalogue.data?.types ?? []}
        value={draft.events}
        disabled={disabled}
        error={errors.events ?? null}
        onChange={(events) => onChange({ ...draft, events }, "events")}
      />
      {hasContentEvents(draft.events) && contentTypes.length > 0 ? (
        <div data-slot="content-type-filter" className="flex flex-col gap-2">
          <div className="flex flex-col gap-0.5">
            <span id={typesLabel} className="text-sm font-medium text-foreground">
              {t("form.contentTypes")}
            </span>
            <span className="text-xs text-muted-foreground">{t("form.contentTypesHint")}</span>
          </div>
          <div role="group" aria-labelledby={typesLabel} className="flex flex-wrap gap-x-6">
            {contentTypes.map((ty) => (
              <TypeOption
                key={ty.name}
                label={ty.label}
                checked={draft.contentTypes.includes(ty.name)}
                disabled={disabled}
                onToggle={(on) =>
                  onChange(
                    {
                      ...draft,
                      contentTypes: on
                        ? [...draft.contentTypes, ty.name]
                        : draft.contentTypes.filter((n) => n !== ty.name),
                    },
                    "content_types",
                  )
                }
              />
            ))}
          </div>
          {errors.content_types ? (
            <p data-slot="form-error" className="text-sm text-destructive">
              {errors.content_types}
            </p>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

function TypeOption({
  label,
  checked,
  disabled,
  onToggle,
}: {
  label: string;
  checked: boolean;
  disabled?: boolean | undefined;
  onToggle: (on: boolean) => void;
}) {
  const id = useId();
  return (
    <div className="flex min-h-11 items-center gap-3">
      <Checkbox
        id={id}
        checked={checked}
        disabled={disabled}
        onCheckedChange={(v) => onToggle(v === true)}
      />
      <Label htmlFor={id} className="font-normal">
        {label}
      </Label>
    </div>
  );
}
