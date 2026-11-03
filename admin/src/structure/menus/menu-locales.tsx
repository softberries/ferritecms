// Menus per locale (M6-1 Task 16): a location's menu in each other locale (a menu with
// that `locale` whose `translation_of` names the location's menu), and "Translate into …"
// in the editor, which makes `<name>-<prefix>` in that locale with a copy of the entries.

import { Link, useNavigate } from "@tanstack/react-router";
import { useQueryClient } from "@tanstack/react-query";
import { PlusIcon } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { reportApiError } from "@/api/client";
import { qk } from "@/api/keys";
import { type MenuDto, putMenu } from "@/api/menus";
import type { SchemaMenuSummary } from "@/api/schema";
import { Button } from "@/components/ui/button";
import type { SiteLocales } from "@/lib/locales";
import { copyEntries, translationOf } from "./menu-locales-model";

/** A location's menus in the other locales, each a link (or "the default menu"). */
export function LocationLocales({
  name,
  menus,
  locales,
}: {
  name: string;
  menus: readonly SchemaMenuSummary[];
  locales: SiteLocales;
}) {
  const { t } = useTranslation("structure");
  if (!locales.multilingual) return null;
  return (
    <ul data-slot="location-locales" className="flex flex-wrap gap-x-3 gap-y-1 text-xs">
      {locales.all
        .filter((l) => l.tag !== locales.default)
        .map((l) => {
          const m = translationOf(menus, name, l.tag);
          return (
            <li key={l.tag} data-locale={l.tag} className="text-muted-foreground">
              <span className="font-mono">{l.tag}</span>{" "}
              {m ? (
                <Link
                  to="/structure/menus/$name"
                  params={{ name: m.name }}
                  className="text-foreground underline-offset-2 hover:underline"
                >
                  {m.label || m.name}
                </Link>
              ) : (
                t("menus.locales.default")
              )}
            </li>
          );
        })}
    </ul>
  );
}

/** "Translate into …" for a default-locale menu: one button per locale without one. */
export function TranslateMenu({
  menu,
  menus,
  locales,
  disabled,
}: {
  menu: MenuDto;
  menus: readonly SchemaMenuSummary[];
  locales: SiteLocales;
  disabled: boolean;
}) {
  const { t } = useTranslation("structure");
  const qc = useQueryClient();
  const navigate = useNavigate();
  const [busy, setBusy] = useState<string | null>(null);
  if (!locales.multilingual || menu.locale !== locales.default) return null;
  const missing = locales.all.filter(
    (l) => l.tag !== locales.default && !translationOf(menus, menu.name, l.tag),
  );
  if (missing.length === 0) return null;
  const translate = async (tag: string, prefix: string) => {
    if (busy) return;
    setBusy(tag);
    const name = `${menu.name}-${prefix}`;
    try {
      const made = await putMenu(name, {
        label: menu.label,
        items: copyEntries(menu.items),
        locale: tag,
        translation_of: menu.name,
      });
      void qc.invalidateQueries({ queryKey: qk.menus.list });
      toast.success(t("locales.menuMade", { name: made.name, locale: locales.label(tag) }));
      void navigate({ to: "/structure/menus/$name", params: { name: made.name } });
    } catch (err) {
      reportApiError(err);
      toast.error(t("locales.failed"));
    } finally {
      setBusy(null);
    }
  };
  return (
    <div data-slot="menu-translate" className="flex flex-wrap gap-2">
      {missing.map((l) => (
        <Button
          key={l.tag}
          type="button"
          variant="outline"
          size="sm"
          disabled={disabled || busy !== null}
          onClick={() => void translate(l.tag, l.prefix ?? l.tag.toLowerCase())}
        >
          <PlusIcon data-icon="inline-start" />
          {busy === l.tag ? t("locales.making") : t("locales.translate", { locale: l.label })}
        </Button>
      ))}
    </div>
  );
}
