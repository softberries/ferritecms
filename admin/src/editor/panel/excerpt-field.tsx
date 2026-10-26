// The excerpt (pre-flight F39): the field shows the excerpt as stored. The server keeps
// a derived excerpt as plain text too, so the admin cannot tell derived from written;
// "Derive from body" empties the field, which the next save sends as `excerpt: null`
// and the server derives it again (160 characters of the body).

import { SparklesIcon } from "lucide-react";
import { useId } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { uiLocale } from "@/lib/locale";
import type { EditorDocument } from "../editor-state";

export function ExcerptField({ doc, disabled }: { doc: EditorDocument; disabled: boolean }) {
  const { t } = useTranslation("editor");
  const id = useId();
  const hintId = useId();
  const value = doc.meta.excerpt;
  const derivePending = value === "" && doc.savedMeta.excerpt !== "";
  const n = new Intl.NumberFormat(uiLocale()).format(value.length);
  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-center justify-between gap-2">
        <Label htmlFor={id}>{t("excerpt.label")}</Label>
        {value !== "" && !disabled ? (
          <Button
            type="button"
            variant="ghost"
            size="xs"
            className="-mr-1.5 text-muted-foreground"
            onClick={() => doc.setMeta({ excerpt: "" })}
          >
            <SparklesIcon data-icon="inline-start" />
            {t("excerpt.derive")}
          </Button>
        ) : null}
      </div>
      <Textarea
        id={id}
        data-slot="excerpt-input"
        rows={3}
        value={value}
        disabled={disabled}
        placeholder={t("excerpt.placeholder")}
        aria-describedby={hintId}
        onChange={(e) => doc.setMeta({ excerpt: e.target.value })}
        className="min-h-20 resize-y text-sm"
      />
      <p id={hintId} className="flex justify-between gap-2 text-xs text-muted-foreground">
        <span>{derivePending ? t("excerpt.willDerive") : t("excerpt.hint")}</span>
        {value !== "" ? (
          <span className="shrink-0 tabular-nums">
            {t("excerpt.count", { count: value.length, formatted: n })}
          </span>
        ) : null}
      </p>
    </div>
  );
}
