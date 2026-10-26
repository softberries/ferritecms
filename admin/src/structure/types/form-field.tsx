// One labelled control of the type screens: label, control, hint and error, wired with
// `aria-describedby` and `aria-invalid` (the control gets them through `children`).

import { useId, type ReactNode } from "react";
import { cn } from "cn";
import { Label } from "@/components/ui/label";

export interface ControlProps {
  id: string;
  "aria-describedby"?: string;
  "aria-invalid"?: true;
}

export function FormField({
  label,
  hint,
  error,
  className,
  children,
  slot,
}: {
  label: ReactNode;
  hint?: ReactNode;
  error?: string | undefined;
  className?: string;
  /** `data-slot` of the wrapper (E2E and the snapshots find fields by it). */
  slot?: string;
  children: (control: ControlProps) => ReactNode;
}) {
  const id = useId();
  const hintId = `${id}-hint`;
  const errorId = `${id}-error`;
  const described = [hint ? hintId : null, error ? errorId : null].filter(Boolean).join(" ");
  return (
    <div data-slot={slot} className={cn("flex min-w-0 flex-col gap-1.5", className)}>
      <Label htmlFor={id}>{label}</Label>
      {children({
        id,
        ...(described ? { "aria-describedby": described } : {}),
        ...(error ? { "aria-invalid": true as const } : {}),
      })}
      {hint ? (
        <p id={hintId} className="text-xs text-muted-foreground">
          {hint}
        </p>
      ) : null}
      {error ? (
        <p id={errorId} data-slot="form-error" className="text-sm text-destructive">
          {error}
        </p>
      ) : null}
    </div>
  );
}

/** A switch with its label and hint beside it (the label is the switch's name). */
export function SwitchRow({
  label,
  hint,
  children,
}: {
  label: ReactNode;
  hint?: ReactNode;
  children: (control: ControlProps) => ReactNode;
}) {
  const id = useId();
  const hintId = `${id}-hint`;
  return (
    <div className="flex items-start justify-between gap-4">
      <div className="flex min-w-0 flex-col gap-1">
        <Label htmlFor={id}>{label}</Label>
        {hint ? (
          <p id={hintId} className="text-xs text-muted-foreground">
            {hint}
          </p>
        ) : null}
      </div>
      {children({ id, ...(hint ? { "aria-describedby": hintId } : {}) })}
    </div>
  );
}
