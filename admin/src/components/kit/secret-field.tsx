// A write-only secret setting (a plugin's `secret`; 07 §1 H-P4): it never holds or shows
// a stored value, only whether one is set. Untouched it reports nothing (`undefined`, so
// the `PATCH` leaves the key out and the server keeps it); Replace (or "Set a value")
// opens an empty password input whose text is the new value; Remove reports `null`
// (delete on save), with Undo. The typed text lives only in this component's state.

import { KeyRoundIcon } from "lucide-react";
import { useId, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { cn } from "cn";

export interface SecretFieldProps {
  /** The setting's label: the input's accessible name. */
  name: string;
  /** Whether a usable value is stored. */
  set: boolean;
  /** `undefined` untouched (keep), a string to set, `null` to remove. */
  onChange: (value: string | null | undefined) => void;
  /** The input's id (for an outer `<label>`); one is made up otherwise. */
  id?: string;
  /** Ids of a hint or an error that describe the input. */
  describedBy?: string;
  invalid?: boolean;
  disabled?: boolean;
}

type Mode = "keep" | "replace" | "remove";

export function SecretField({
  name,
  set,
  onChange,
  id,
  describedBy,
  invalid = false,
  disabled = false,
}: SecretFieldProps) {
  const { t } = useTranslation("admin");
  const ownId = useId();
  const inputId = id ?? ownId;
  const [mode, setMode] = useState<Mode>("keep");
  const [text, setText] = useState("");
  const to = (next: Mode) => {
    setMode(next);
    setText("");
    onChange(next === "remove" ? null : undefined);
  };

  if (mode === "replace") {
    return (
      <div data-slot="secret-field" data-state="replace" className="flex min-w-0 flex-col gap-2">
        <div className="flex min-w-0 gap-2">
          <Input
            id={inputId}
            type="password"
            autoComplete="new-password"
            spellCheck={false}
            // Opened by a click on Replace: the input is what the user acts on next.
            autoFocus
            aria-label={id ? undefined : name}
            aria-describedby={describedBy}
            aria-invalid={invalid || undefined}
            disabled={disabled}
            value={text}
            onChange={(e) => {
              setText(e.target.value);
              onChange(e.target.value === "" ? undefined : e.target.value);
            }}
            className="min-w-0 flex-1 font-mono"
          />
          <Button
            type="button"
            variant="ghost"
            disabled={disabled}
            onClick={() => {
              to("keep");
            }}
          >
            {t("secret.cancel")}
          </Button>
        </div>
        <p className="text-xs text-muted-foreground">
          {set ? t("secret.replaceHint") : t("secret.setHint")}
        </p>
      </div>
    );
  }

  const removing = mode === "remove";
  return (
    <div
      data-slot="secret-field"
      data-state={removing ? "remove" : set ? "set" : "unset"}
      className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-2"
    >
      <span
        className={cn(
          "inline-flex min-h-(--control-h) items-center gap-1.5 text-sm",
          removing
            ? "text-destructive"
            : set
              ? "font-medium text-foreground"
              : "text-muted-foreground",
        )}
      >
        <KeyRoundIcon aria-hidden="true" className="size-3.5 shrink-0" />
        {removing ? t("secret.removing") : set ? t("secret.set") : t("secret.notSet")}
      </span>
      <div className="flex gap-2">
        {removing ? (
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={disabled}
            onClick={() => {
              to("keep");
            }}
          >
            {t("secret.undo")}
          </Button>
        ) : (
          <>
            <Button
              type="button"
              variant="outline"
              size="sm"
              disabled={disabled}
              aria-describedby={describedBy}
              onClick={() => {
                to("replace");
              }}
            >
              {set ? t("secret.replace") : t("secret.add")}
            </Button>
            {set ? (
              <Button
                type="button"
                variant="ghost"
                size="sm"
                disabled={disabled}
                onClick={() => {
                  to("remove");
                }}
                className="text-muted-foreground"
              >
                {t("secret.remove")}
              </Button>
            ) : null}
          </>
        )}
      </div>
    </div>
  );
}
