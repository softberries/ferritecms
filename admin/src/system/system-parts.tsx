// Small pieces the System tabs share: a server-sent text that expands when long (job
// errors, node errors: always text nodes, never markup), a tone icon, and a fact row.

import { cn } from "cn";
import { CircleAlertIcon, CircleCheckIcon, TriangleAlertIcon } from "lucide-react";
import { type ReactNode, useState } from "react";
import { useTranslation } from "react-i18next";
import type { Tone } from "./system-model";

/** Texts longer than this are clamped to two lines with a toggle. */
export const LONG_TEXT = 120;

export function ExpandableText({ text, className }: { text: string; className?: string }) {
  const { t } = useTranslation("admin");
  const [open, setOpen] = useState(false);
  const long = text.length > LONG_TEXT;
  return (
    <div data-slot="expandable-text" className="flex min-w-0 flex-col items-start gap-1">
      <p
        className={cn(
          "min-w-0 font-mono text-xs whitespace-pre-wrap text-muted-foreground wrap-anywhere",
          long && !open && "line-clamp-2",
          className,
        )}
      >
        {text}
      </p>
      {long ? (
        <button
          type="button"
          aria-expanded={open}
          onClick={() => setOpen((o) => !o)}
          className="-mx-1 rounded-sm px-1 text-xs font-medium text-primary hover:underline pointer-coarse:min-h-11"
        >
          {open ? t("system.jobs.error.less") : t("system.jobs.error.more")}
        </button>
      ) : null}
    </div>
  );
}

const TONE: Record<Tone, { icon: typeof CircleCheckIcon; className: string }> = {
  ok: { icon: CircleCheckIcon, className: "text-success" },
  warn: { icon: TriangleAlertIcon, className: "text-warning" },
  bad: { icon: CircleAlertIcon, className: "text-destructive" },
};

/** A tone as an icon (decorative: put the words next to it, never colour alone). */
export function ToneIcon({ tone, className }: { tone: Tone; className?: string }) {
  const { icon: Icon, className: colour } = TONE[tone];
  return <Icon aria-hidden="true" className={cn("size-4 shrink-0", colour, className)} />;
}

/** One `<dt>`/`<dd>` pair of a facts list. */
export function Fact({
  label,
  children,
  className,
}: {
  label: ReactNode;
  children: ReactNode;
  className?: string;
}) {
  return (
    <div className={cn("flex min-w-0 flex-col gap-0.5", className)}>
      <dt className="text-xs text-muted-foreground">{label}</dt>
      <dd className="min-w-0 text-sm text-foreground wrap-anywhere">{children}</dd>
    </div>
  );
}
