// A choice of a few options, each a card with a title and an explanation: native radios
// in a labelled `radiogroup` (arrow keys move between them).

import { useId, type ReactNode } from "react";
import { cn } from "cn";

export interface Choice<V extends string> {
  value: V;
  title: ReactNode;
  description?: ReactNode;
  /** Shown after the title (an example path). */
  aside?: ReactNode;
}

export function ChoiceCards<V extends string>({
  label,
  value,
  choices,
  onChange,
  disabled = false,
  columns = 1,
  slot,
}: {
  label: string;
  value: V;
  choices: readonly Choice<V>[];
  onChange: (value: V) => void;
  disabled?: boolean;
  columns?: 1 | 2;
  slot?: string;
}) {
  const id = useId();
  return (
    <div className="flex min-w-0 flex-col gap-2">
      <span id={`${id}-label`} className="text-sm font-medium text-foreground">
        {label}
      </span>
      <div
        role="radiogroup"
        aria-labelledby={`${id}-label`}
        data-slot={slot}
        className={cn("grid gap-2", columns === 2 && "sm:grid-cols-2")}
      >
        {choices.map((c) => {
          const inputId = `${id}-${c.value}`;
          return (
            <label
              key={c.value}
              htmlFor={inputId}
              data-choice={c.value}
              className={cn(
                "flex min-h-11 min-w-0 cursor-pointer items-start gap-3 rounded-lg border px-3 py-2.5 transition-colors",
                "hover:bg-muted/60 has-checked:border-primary has-checked:bg-primary/5",
                "has-focus-visible:outline-2 has-focus-visible:outline-offset-2 has-focus-visible:outline-ring",
                "has-disabled:cursor-not-allowed has-disabled:opacity-60",
              )}
            >
              <input
                id={inputId}
                type="radio"
                name={id}
                value={c.value}
                checked={value === c.value}
                disabled={disabled}
                onChange={() => onChange(c.value)}
                className="mt-0.5 size-4 shrink-0 accent-primary focus-visible:outline-none"
              />
              <span className="flex min-w-0 flex-1 flex-col gap-0.5">
                <span className="flex min-w-0 flex-wrap items-baseline justify-between gap-x-3 gap-y-0.5">
                  <span className="text-sm font-medium text-foreground">{c.title}</span>
                  {c.aside ? (
                    <span className="min-w-0 font-mono text-xs text-muted-foreground wrap-anywhere">
                      {c.aside}
                    </span>
                  ) : null}
                </span>
                {c.description ? (
                  <span className="text-sm text-muted-foreground">{c.description}</span>
                ) : null}
              </span>
            </label>
          );
        })}
      </div>
    </div>
  );
}
