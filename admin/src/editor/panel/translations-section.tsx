// The editor panel's Translations (M6-1, design 13 §6): the item's language, its
// translations (each a link to its editor, with its status) and, for every other
// language of the site, "Translate", which makes a draft copy in that language
// (`POST /content/{type}/{id}/translations`) and opens it. Hidden while the site has
// one language, and for an item not saved yet.

import { useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import { LanguagesIcon, PlusIcon } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { isContentStatus, translateContent } from "@/api/content";
import { reportApiError } from "@/api/client";
import { qk } from "@/api/keys";
import { StatusDot } from "@/components/kit/status-badge";
import { Button } from "@/components/ui/button";
import { useSiteLocales } from "@/lib/locales";
import type { EditorDocument } from "../editor-state";

export function TranslationsSection({ doc, type }: { doc: EditorDocument; type: string }) {
  const { t } = useTranslation("editor");
  const { t: tc } = useTranslation();
  const locales = useSiteLocales();
  const navigate = useNavigate();
  const qc = useQueryClient();
  const [busy, setBusy] = useState<string | null>(null);
  const item = doc.item;
  if (!locales.multilingual || !item) return null;
  const byLocale = new Map(item.translations.map((tr) => [tr.locale, tr]));
  const missing = locales.all.filter((l) => l.tag !== item.locale && !byLocale.has(l.tag));
  const canTranslate = item.allowed.edit && doc.readOnly === null;

  const translate = async (locale: string) => {
    if (busy) return;
    setBusy(locale);
    try {
      const made = await translateContent(type, item.id, locale);
      await Promise.all([
        qc.invalidateQueries({ queryKey: qk.content.lists(type) }),
        qc.invalidateQueries({ queryKey: qk.content.counts(type) }),
      ]);
      const notes = made.notes.map((n) => t(`translations.notes.${noteKey(n)}`));
      toast.success(t("translations.made", { locale: locales.label(locale) }), {
        ...(notes.length > 0 ? { description: notes.join(" ") } : {}),
      });
      void navigate({ to: "/content/$type/$id", params: { type, id: made.item.id } });
    } catch (err) {
      reportApiError(err);
      toast.error(t("translations.failed"));
    } finally {
      setBusy(null);
    }
  };

  return (
    <section
      aria-labelledby="panel-translations"
      data-slot="panel-translations"
      className="flex flex-col gap-3"
    >
      <h3 id="panel-translations" className="flex items-center gap-2 text-sm font-semibold">
        <LanguagesIcon aria-hidden="true" className="size-4 text-muted-foreground" />
        {t("translations.heading")}
      </h3>
      <p className="text-sm text-muted-foreground">
        {t("translations.language", { locale: locales.label(item.locale), tag: item.locale })}
      </p>
      {item.translations.length > 0 ? (
        <ul className="flex flex-col gap-1">
          {item.translations.map((tr) => (
            <li key={tr.id} className="flex min-w-0 items-center gap-2 text-sm">
              <span className="w-12 shrink-0 font-mono text-xs text-muted-foreground">
                {tr.locale}
              </span>
              <Link
                to="/content/$type/$id"
                params={{ type, id: tr.id }}
                className="min-w-0 flex-1 truncate font-medium text-foreground hover:underline"
                title={tr.title}
              >
                {tr.title || t("translations.untitled")}
              </Link>
              {isContentStatus(tr.status) ? (
                <span className="flex shrink-0 items-center gap-1 text-xs text-muted-foreground">
                  <StatusDot status={tr.status} />
                  {tc(`status.${tr.status}`)}
                </span>
              ) : null}
            </li>
          ))}
        </ul>
      ) : null}
      {canTranslate && missing.length > 0 ? (
        <div className="flex flex-wrap gap-2">
          {missing.map((l) => (
            <Button
              key={l.tag}
              type="button"
              variant="outline"
              size="sm"
              disabled={busy !== null || doc.dirty}
              title={doc.dirty ? t("translations.saveFirst") : undefined}
              onClick={() => void translate(l.tag)}
            >
              <PlusIcon data-icon="inline-start" />
              {busy === l.tag
                ? t("translations.making")
                : t("translations.translate", { locale: l.label })}
            </Button>
          ))}
        </div>
      ) : null}
      {canTranslate && missing.length > 0 && doc.dirty ? (
        <p className="text-xs text-muted-foreground">{t("translations.saveFirst")}</p>
      ) : null}
    </section>
  );
}

/** The i18n key of a server note (`term_untranslated:<id>` → `termUntranslated`). */
function noteKey(note: string): "parentUntranslated" | "termUntranslated" | "other" {
  if (note === "parent_untranslated") return "parentUntranslated";
  if (note.startsWith("term_untranslated")) return "termUntranslated";
  return "other";
}
