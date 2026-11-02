// A value to copy that is not secret (the MCP URL, a command, a config snippet): a
// read-only mono field or a code block, each with Copy. The outcome is said in place
// (the button's label and a polite live region), so it works outside the shell's
// toaster too.

import { cn } from "cn";
import { CheckIcon, CopyIcon } from "lucide-react";
import { useId } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { type CopyState, useCopy } from "./use-copy";

function CopyButton({
  state,
  copy,
  label,
  className,
}: {
  state: CopyState;
  copy: () => void;
  /** What is copied, for the accessible name ("Copy MCP URL"). */
  label: string;
  className?: string;
}) {
  const { t } = useTranslation("kit");
  return (
    <Button
      type="button"
      variant="outline"
      aria-label={state === "copied" ? t("copy.copied") : t("copy.copyLabel", { label })}
      onClick={copy}
      className={className}
    >
      {state === "copied" ? (
        <CheckIcon data-icon="inline-start" className="text-success" />
      ) : (
        <CopyIcon data-icon="inline-start" />
      )}
      {state === "copied" ? t("copy.copied") : t("copy.copy")}
    </Button>
  );
}

function CopyStatus({ state }: { state: CopyState }) {
  const { t } = useTranslation("kit");
  return (
    <>
      <span role="status" className="sr-only">
        {state === "copied" ? t("copy.copiedSr") : ""}
      </span>
      {state === "failed" ? (
        <p role="alert" className="text-sm text-destructive">
          {t("copy.failed")}
        </p>
      ) : null}
    </>
  );
}

export interface CopyFieldProps {
  value: string;
  label: string;
  /** Under the field. */
  hint?: string;
  /** The label is for screen readers only (a card that names the value already). */
  hideLabel?: boolean;
  className?: string;
}

/** A read-only mono field with Copy; focusing the field selects the value. */
export function CopyField({ value, label, hint, hideLabel = false, className }: CopyFieldProps) {
  const id = useId();
  const { state, copy } = useCopy(value);
  return (
    <div data-slot="copy-field" className={cn("flex min-w-0 flex-col gap-2", className)}>
      <Label htmlFor={id} className={hideLabel ? "sr-only" : undefined}>
        {label}
      </Label>
      <div className="flex min-w-0 gap-2">
        <Input
          id={id}
          readOnly
          value={value}
          title={value}
          spellCheck={false}
          autoComplete="off"
          aria-describedby={hint ? `${id}-hint` : undefined}
          onFocus={(e) => e.currentTarget.select()}
          className="min-w-0 flex-1 font-mono text-[0.8125rem]"
        />
        <CopyButton state={state} copy={copy} label={label} />
      </div>
      {hint ? (
        <p id={`${id}-hint`} className="text-sm text-muted-foreground">
          {hint}
        </p>
      ) : null}
      <CopyStatus state={state} />
    </div>
  );
}

/**
 * A command or config snippet: a scrollable code block (keyboard-focusable, so a long
 * line can be scrolled without a mouse) with Copy in its corner.
 */
export function CopyCode({
  value,
  label,
  className,
}: {
  value: string;
  /** What the code is ("Command", "~/.cursor/mcp.json"); names the region and Copy. */
  label: string;
  className?: string;
}) {
  const { state, copy } = useCopy(value);
  return (
    <div data-slot="copy-code" className={cn("flex min-w-0 flex-col gap-2", className)}>
      <div className="relative min-w-0 rounded-lg border bg-muted/60">
        <div className="flex items-center justify-between gap-3 border-b py-1 pr-1 pl-3">
          <span className="truncate font-mono text-xs text-muted-foreground" title={label}>
            {label}
          </span>
          <CopyButton
            state={state}
            copy={copy}
            label={label}
            className="h-8 bg-card pointer-coarse:h-11"
          />
        </div>
        <pre
          tabIndex={0}
          role="region"
          aria-label={label}
          className="overflow-x-auto px-3 py-2.5 font-mono text-[0.8125rem] leading-relaxed text-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring"
        >
          <code>{value}</code>
        </pre>
      </div>
      <CopyStatus state={state} />
    </div>
  );
}
