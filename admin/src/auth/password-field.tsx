// A password input with a show/hide toggle and an optional live length hint against
// the policy (07 §2: at least 12 characters, at most 1024 bytes).

import { CheckIcon, EyeIcon, EyeOffIcon } from "lucide-react";
import { cn } from "cn";
import { type ComponentProps, type ReactNode, useState } from "react";
import { type Control, type FieldValues, type Path, useController } from "react-hook-form";
import { useTranslation } from "react-i18next";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { FieldMessage, authInputClass } from "./auth-layout";
import { MIN_PASSWORD_CHARS, charCount } from "./errors";
import { fieldMessage } from "./schemas";

export interface PasswordFieldProps extends Omit<
  ComponentProps<"input">,
  "id" | "type" | "value" | "onChange"
> {
  id: string;
  label: string;
  value: string;
  onValueChange: (value: string) => void;
  autoComplete: "current-password" | "new-password";
  error?: string | null;
  /** Right of the label (the "Forgot password?" link). */
  aside?: ReactNode;
  /** Shows the live length hint and the policy text (for a new password). */
  policy?: boolean;
}

export function PasswordField({
  id,
  label,
  value,
  onValueChange,
  error,
  aside,
  policy = false,
  className,
  ...props
}: PasswordFieldProps) {
  const { t } = useTranslation("auth");
  const [shown, setShown] = useState(false);
  const count = charCount(value);
  const long = count >= MIN_PASSWORD_CHARS;
  const describedBy =
    [error ? `${id}-error` : null, policy ? `${id}-policy` : null].filter(Boolean).join(" ") ||
    undefined;

  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-center justify-between gap-3">
        <Label htmlFor={id}>{label}</Label>
        {aside}
      </div>
      <div className="relative">
        <Input
          id={id}
          type={shown ? "text" : "password"}
          value={value}
          onChange={(e) => {
            onValueChange(e.target.value);
          }}
          spellCheck={false}
          autoCapitalize="none"
          autoCorrect="off"
          aria-invalid={error ? true : undefined}
          aria-describedby={describedBy}
          className={cn(authInputClass, "pr-12", className)}
          {...props}
        />
        <button
          type="button"
          aria-label={shown ? t("password.hide") : t("password.show")}
          aria-pressed={shown}
          aria-controls={id}
          onClick={() => {
            setShown((s) => !s);
          }}
          className="absolute inset-y-0 right-0 flex w-11 items-center justify-center rounded-r-lg text-muted-foreground transition-colors -outline-offset-2 hover:text-foreground"
        >
          {shown ? (
            <EyeOffIcon aria-hidden="true" className="size-4" />
          ) : (
            <EyeIcon aria-hidden="true" className="size-4" />
          )}
        </button>
      </div>
      {/* The error sits right under its input, above the length hint and policy. */}
      <FieldMessage id={id} error={error} />
      {policy ? (
        <div id={`${id}-policy`} className="flex flex-col gap-1.5">
          <LengthMeter count={count} />
          <p className="flex items-center gap-1.5 text-sm text-muted-foreground tabular-nums">
            {long ? <CheckIcon aria-hidden="true" className="size-3.5 text-success" /> : null}
            {long
              ? t("policy.ok", { count })
              : t("policy.count", { count, remaining: MIN_PASSWORD_CHARS - count })}
          </p>
          <p className="text-xs text-muted-foreground">{t("policy.text")}</p>
        </div>
      ) : null}
    </div>
  );
}

export type FormPasswordFieldProps<T extends FieldValues> = Omit<
  PasswordFieldProps,
  "value" | "onValueChange" | "error" | "name" | "ref" | "onBlur"
> & { control: Control<T>; name: Path<T> };

/** A `PasswordField` bound to a react-hook-form field (value, focus, error). */
export function FormPasswordField<T extends FieldValues>({
  control,
  name,
  ...props
}: FormPasswordFieldProps<T>) {
  const { t } = useTranslation("auth");
  const {
    field: { ref, name: fieldName, value, onBlur, onChange },
    fieldState: { error },
  } = useController({ control, name });
  return (
    <PasswordField
      {...props}
      ref={ref}
      name={fieldName}
      value={typeof value === "string" ? value : ""}
      onBlur={onBlur}
      onValueChange={onChange}
      error={fieldMessage(t, error)}
    />
  );
}

/** Twelve ticks that fill as the password grows; ember while short, success once long. */
function LengthMeter({ count }: { count: number }) {
  return (
    <div aria-hidden="true" className="grid grid-cols-12 gap-1">
      {Array.from({ length: MIN_PASSWORD_CHARS }, (_, i) => (
        <span
          key={i}
          className={cn(
            "h-1 rounded-full transition-colors duration-(--dur-fast)",
            i < count ? (count >= MIN_PASSWORD_CHARS ? "bg-success" : "bg-primary") : "bg-border",
          )}
        />
      ))}
    </div>
  );
}
