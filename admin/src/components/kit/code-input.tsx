// The six-digit code input of TOTP sign-in and enrolment: one real input
// (`inputmode="numeric"`, `autocomplete="one-time-code"`, so SMS and password-manager
// autofill work) laid over six drawn cells in two groups of three.

import { cn } from "cn";
import { type Ref, useState } from "react";

export const CODE_DIGITS = 6;

export interface CodeInputProps {
  ref?: Ref<HTMLInputElement>;
  /** The input's id, for a `<label htmlFor>` (`totp-code` on the sign-in screen). */
  id?: string;
  value: string;
  /** Digits only, at most six. */
  onChange: (value: string) => void;
  /** Called once the sixth digit is in (the sign-in screen submits then). */
  onComplete?: (value: string) => void;
  /** The accessible name when no `<label>` points at `id`. */
  label?: string;
  /** Ids of a hint or an error that describe the input. */
  describedBy?: string;
  invalid?: boolean;
  disabled?: boolean;
  autoFocus?: boolean;
}

export function CodeInput({
  ref,
  id,
  value,
  onChange,
  onComplete,
  label,
  describedBy,
  invalid = false,
  disabled = false,
  autoFocus = true,
}: CodeInputProps) {
  const [focused, setFocused] = useState(false);
  const cells = Array.from({ length: CODE_DIGITS }, (_, i) => value[i] ?? "");
  const active = Math.min(value.length, CODE_DIGITS - 1);
  const cell = (i: number) => (
    <span
      key={i}
      className={cn(
        "flex h-12 w-11 items-center justify-center rounded-lg border bg-background font-mono text-xl font-medium text-foreground tabular-nums transition-colors duration-(--dur-instant) dark:bg-input/30",
        invalid ? "border-destructive" : "border-input",
        focused && i === active && !invalid && "border-primary ring-2 ring-primary/25",
      )}
    >
      {cells[i] ? (
        cells[i]
      ) : focused && i === active ? (
        <span className="h-6 w-px bg-foreground" />
      ) : null}
    </span>
  );
  return (
    <div className="relative w-full rounded-xl has-[input:focus-visible]:outline-2 has-[input:focus-visible]:outline-offset-4 has-[input:focus-visible]:outline-ring">
      <div aria-hidden="true" className="flex items-center justify-between">
        <div className="flex gap-2">{[0, 1, 2].map(cell)}</div>
        <span className="h-px w-3 bg-input" />
        <div className="flex gap-2">{[3, 4, 5].map(cell)}</div>
      </div>
      <input
        ref={ref}
        id={id}
        name="code"
        type="text"
        inputMode="numeric"
        autoComplete="one-time-code"
        pattern="[0-9]*"
        maxLength={CODE_DIGITS}
        autoFocus={autoFocus}
        disabled={disabled}
        aria-label={label}
        aria-invalid={invalid || undefined}
        aria-describedby={describedBy}
        value={value}
        onFocus={() => {
          setFocused(true);
        }}
        onBlur={() => {
          setFocused(false);
        }}
        onChange={(e) => {
          const next = e.target.value.replace(/\D/g, "").slice(0, CODE_DIGITS);
          onChange(next);
          if (next.length === CODE_DIGITS && next !== value) onComplete?.(next);
        }}
        className="absolute inset-0 size-full cursor-text bg-transparent font-mono text-transparent caret-transparent opacity-0 outline-none selection:bg-transparent disabled:cursor-not-allowed"
      />
    </div>
  );
}
