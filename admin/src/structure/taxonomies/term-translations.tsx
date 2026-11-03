// The term sheet's Translations (M6-1 Task 16): the term's other locales (each with its
// name) and, for a locale the group has no term in, "Translate into …", which makes the
// term in that locale with the same name and slug (`POST /terms/{id}/translations`).
// Nothing on a one-locale site.

import { useQueryClient } from "@tanstack/react-query";
import { PlusIcon } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { reportApiError } from "@/api/client";
import { qk } from "@/api/keys";
import type { TermDto } from "@/api/taxonomies";
import { translateTerm } from "@/api/terms";
import { Button } from "@/components/ui/button";
import { useSiteLocales } from "@/lib/locales";

export function TermTranslations({
  term,
  allTerms,
  disabled,
}: {
  term: TermDto;
  /** Every term of the taxonomy, in every locale. */
  allTerms: readonly TermDto[];
  disabled: boolean;
}) {
  const { t } = useTranslation("structure");
  const locales = useSiteLocales();
  const qc = useQueryClient();
  const [busy, setBusy] = useState<string | null>(null);
  if (!locales.multilingual) return null;
  const members = allTerms.filter(
    (x) => x.translation_group === term.translation_group && x.id !== term.id,
  );
  const missing = locales.all.filter(
    (l) => l.tag !== term.locale && !members.some((m) => m.locale === l.tag),
  );
  const translate = async (tag: string) => {
    if (busy) return;
    setBusy(tag);
    try {
      const made = await translateTerm(term.id, tag);
      qc.setQueryData<TermDto[]>(qk.terms(term.taxonomy), (list) =>
        list ? [...list, made] : list,
      );
      void qc.invalidateQueries({ queryKey: qk.terms(term.taxonomy) });
      toast.success(t("locales.termMade", { name: made.name, locale: locales.label(tag) }));
    } catch (err) {
      reportApiError(err);
      toast.error(t("locales.failed"));
    } finally {
      setBusy(null);
    }
  };
  return (
    <section
      data-slot="term-translations"
      aria-labelledby="term-translations-title"
      className="flex flex-col gap-2"
    >
      <h3 id="term-translations-title" className="text-sm font-medium">
        {t("locales.translations")}
      </h3>
      <p className="text-sm text-muted-foreground">
        {t("locales.language", { locale: locales.label(term.locale), tag: term.locale })}
      </p>
      {members.length > 0 ? (
        <ul className="flex flex-col gap-1 text-sm">
          {members.map((m) => (
            <li key={m.id} className="flex items-center gap-2">
              <span className="w-12 shrink-0 font-mono text-xs text-muted-foreground">
                {m.locale}
              </span>
              <span className="truncate">{m.name}</span>
            </li>
          ))}
        </ul>
      ) : null}
      {missing.length > 0 ? (
        <div className="flex flex-wrap gap-2">
          {missing.map((l) => (
            <Button
              key={l.tag}
              type="button"
              size="sm"
              variant="outline"
              disabled={disabled || busy !== null}
              onClick={() => void translate(l.tag)}
            >
              <PlusIcon data-icon="inline-start" />
              {busy === l.tag ? t("locales.making") : t("locales.translate", { locale: l.label })}
            </Button>
          ))}
        </div>
      ) : null}
    </section>
  );
}
