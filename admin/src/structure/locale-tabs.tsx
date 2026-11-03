// The language picker of the taxonomy and menu screens (M6-1 Task 16): one toggle per
// site locale, the default first. Nothing on a one-locale site.

import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import type { SiteLocales } from "@/lib/locales";

export function LocaleTabs({
  locales,
  value,
  onChange,
}: {
  locales: SiteLocales;
  value: string;
  onChange: (tag: string) => void;
}) {
  const { t } = useTranslation("structure");
  if (!locales.multilingual) return null;
  return (
    <div
      role="group"
      aria-label={t("locales.pick")}
      data-slot="locale-tabs"
      className="flex flex-wrap items-center gap-1"
    >
      {locales.all.map((l) => (
        <Button
          key={l.tag}
          type="button"
          size="sm"
          variant={l.tag === value ? "secondary" : "ghost"}
          aria-pressed={l.tag === value}
          data-locale={l.tag}
          onClick={() => onChange(l.tag)}
        >
          {l.label}
          <span className="font-mono text-xs text-muted-foreground">{l.tag}</span>
        </Button>
      ))}
    </div>
  );
}
