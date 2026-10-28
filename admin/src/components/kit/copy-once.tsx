// A value shown once (a new API token's bearer value; 07 §1 H-A8): a read-only mono
// field with Copy and the warning that it will not be shown again. The value lives only
// in the caller's component state; this component never stores it elsewhere.

import { CheckIcon, CopyIcon, TriangleAlertIcon } from "lucide-react";
import { useEffect, useId, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";

export interface CopyOnceProps {
  value: string;
  /** The field's label ("Token"). */
  label: string;
}

export function CopyOnce({ value, label }: CopyOnceProps) {
  const { t } = useTranslation("admin");
  const id = useId();
  const hintId = `${id}-hint`;
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const timer = window.setTimeout(() => setCopied(false), 1_500);
    return () => window.clearTimeout(timer);
  }, [copied]);

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(value);
      setCopied(true);
      toast.success(t("copyOnce.copied"), { id: "copy-once" });
    } catch {
      toast.error(t("copyOnce.copyFailed"), { id: "copy-once" });
    }
  };

  return (
    <div data-slot="copy-once" className="flex min-w-0 flex-col gap-2">
      <Label htmlFor={id}>{label}</Label>
      <div className="flex min-w-0 gap-2">
        <Input
          id={id}
          readOnly
          value={value}
          spellCheck={false}
          autoComplete="off"
          aria-describedby={hintId}
          onFocus={(e) => {
            e.currentTarget.select();
          }}
          className="min-w-0 flex-1 font-mono text-[0.8125rem]"
        />
        <Button
          type="button"
          variant="outline"
          onClick={() => {
            void copy();
          }}
        >
          {copied ? <CheckIcon data-icon="inline-start" /> : <CopyIcon data-icon="inline-start" />}
          {copied ? t("copyOnce.copiedShort") : t("copyOnce.copy")}
        </Button>
      </div>
      <p id={hintId} className="flex items-start gap-1.5 text-sm text-warning">
        <TriangleAlertIcon aria-hidden="true" className="mt-0.5 size-3.5 shrink-0" />
        {t("copyOnce.hint")}
      </p>
    </div>
  );
}
