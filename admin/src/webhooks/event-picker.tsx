// The subscription picker (design 18 §10.1): "All events", and per group "All <group>
// events" plus its single types, fed by `GET /webhooks/events`. The value is the list of
// patterns the API takes (`*`, `<group>.*`, exact types); a broader choice covers the
// narrower ones, which are shown checked and locked.

import { cn } from "cn";
import { useId } from "react";
import { useTranslation } from "react-i18next";
import { Checkbox } from "@/components/ui/checkbox";
import { Label } from "@/components/ui/label";
import { type EventType, covers, groupCatalogue, groupLabel, groupOf } from "./webhooks-model";

export interface EventPickerProps {
  types: readonly EventType[];
  value: readonly string[];
  onChange: (next: string[]) => void;
  disabled?: boolean;
  error?: string | null;
}

function Option({
  label,
  hint,
  checked,
  locked,
  disabled,
  onToggle,
  strong,
}: {
  label: string;
  hint?: string;
  checked: boolean;
  locked?: boolean;
  disabled?: boolean;
  onToggle: (on: boolean) => void;
  strong?: boolean;
}) {
  const id = useId();
  const hintId = `${id}-hint`;
  return (
    <div className="flex min-h-11 items-start gap-3 py-1.5">
      <Checkbox
        id={id}
        className="mt-0.5"
        checked={checked}
        disabled={disabled || locked}
        aria-describedby={hint ? hintId : undefined}
        onCheckedChange={(v) => onToggle(v === true)}
      />
      <div className="flex min-w-0 flex-col items-start gap-0.5">
        <Label
          htmlFor={id}
          className={cn("text-sm font-normal text-foreground", strong && "font-medium")}
        >
          {label}
        </Label>
        {hint ? (
          <span id={hintId} className="text-xs text-muted-foreground">
            {hint}
          </span>
        ) : null}
      </div>
    </div>
  );
}

export function EventPicker({ types, value, onChange, disabled, error }: EventPickerProps) {
  const { t } = useTranslation("webhooks");
  const labelId = useId();
  const all = value.includes("*");
  const groups = groupCatalogue(types);

  const set = (pattern: string, on: boolean) => {
    if (pattern === "*") {
      onChange(on ? ["*"] : []);
      return;
    }
    if (pattern.endsWith(".*")) {
      const g = groupOf(pattern);
      // The group replaces its single types.
      const rest = value.filter((p) => groupOf(p) !== g);
      onChange(on ? [...rest, pattern] : rest);
      return;
    }
    onChange(on ? [...value, pattern] : value.filter((p) => p !== pattern));
  };

  return (
    <div data-slot="event-picker" className="flex min-w-0 flex-col gap-2">
      <div className="flex flex-col gap-0.5">
        <span id={labelId} className="text-sm font-medium text-foreground">
          {t("events.label")}
        </span>
        <span className="text-xs text-muted-foreground">{t("events.hint")}</span>
      </div>
      <div
        role="group"
        aria-labelledby={labelId}
        aria-invalid={error ? true : undefined}
        className={cn(
          "max-h-[45svh] divide-y overflow-y-auto rounded-lg border px-3",
          error && "border-destructive",
        )}
      >
        <Option
          label={t("events.all")}
          checked={all}
          disabled={disabled}
          strong
          onToggle={(on) => set("*", on)}
        />
        {groups.map(({ group, types: inGroup }) => {
          const name = groupLabel(t, group);
          const groupOn = all || value.includes(`${group}.*`);
          return (
            <fieldset key={group} data-group={group} className="flex flex-col py-1">
              <legend className="sr-only">{name}</legend>
              <Option
                label={t("events.allInGroup", { group: name })}
                checked={groupOn}
                locked={all}
                disabled={disabled}
                strong
                onToggle={(on) => set(`${group}.*`, on)}
              />
              <div className="flex flex-col pl-7">
                {inGroup.map((type) => (
                  <Option
                    key={type.type}
                    label={type.type}
                    hint={type.description}
                    checked={covers(value, type.type)}
                    locked={groupOn}
                    disabled={disabled}
                    onToggle={(on) => set(type.type, on)}
                  />
                ))}
              </div>
            </fieldset>
          );
        })}
      </div>
      {error ? (
        <p data-slot="form-error" className="text-sm text-destructive">
          {error}
        </p>
      ) : null}
    </div>
  );
}
