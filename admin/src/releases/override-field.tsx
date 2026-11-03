// "Override approval" (design 15 §6.5, D21): a disclosure for admins in the publish,
// schedule and apply dialogs. Checking it asks for a reason (1–500 characters), which the
// server writes to the audit log (`workflow.override`); the confirm button stays disabled
// until there is one. The admin never decides who may override: it shows this only where
// the server said so (`allowed.override`, or the admin role for a release).

import { useId } from "react";
import { useTranslation } from "react-i18next";
import { MAX_OVERRIDE_REASON } from "@/api/releases";
import { Checkbox } from "@/components/ui/checkbox";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { type OverrideState, overrideIncomplete } from "./override";

export function OverrideField({
  value,
  onChange,
  disabled,
}: {
  value: OverrideState;
  onChange: (next: OverrideState) => void;
  disabled?: boolean;
}) {
  const { t } = useTranslation("releases");
  const checkId = useId();
  const reasonId = useId();
  const hintId = useId();
  return (
    <details
      data-slot="override"
      className="group rounded-lg border border-warning/40 bg-warning/6 px-3 py-2 text-sm"
    >
      <summary className="flex min-h-9 cursor-pointer items-center font-medium text-foreground">
        {t("override.summary")}
      </summary>
      <div className="flex flex-col gap-3 pt-1 pb-2">
        <p id={hintId} className="text-muted-foreground">
          {t("override.hint")}
        </p>
        <div className="flex items-center gap-2">
          <Checkbox
            id={checkId}
            checked={value.on}
            disabled={disabled}
            onCheckedChange={(c) => onChange({ ...value, on: c === true })}
          />
          <Label htmlFor={checkId}>{t("override.check")}</Label>
        </div>
        {value.on ? (
          <div className="flex flex-col gap-1.5">
            <Label htmlFor={reasonId}>{t("override.reason")}</Label>
            <Textarea
              id={reasonId}
              value={value.reason}
              maxLength={MAX_OVERRIDE_REASON}
              required
              disabled={disabled}
              aria-describedby={hintId}
              aria-invalid={overrideIncomplete(value) ? true : undefined}
              onChange={(e) => onChange({ ...value, reason: e.target.value })}
              rows={2}
            />
            {overrideIncomplete(value) ? (
              <p className="text-xs text-muted-foreground">{t("override.reasonRequired")}</p>
            ) : null}
          </div>
        ) : null}
      </div>
    </details>
  );
}
