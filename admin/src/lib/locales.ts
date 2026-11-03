// The site's locales (M6-1, design 13 §6): `GET /meta` lists them, the default first.
// Every locale control in the admin is hidden while the site has one locale.

import { useQuery } from "@tanstack/react-query";
import { metaQuery } from "@/api/meta";
import type { SchemaMetaLocaleDto } from "@/api/schema";

export type SiteLocale = SchemaMetaLocaleDto;

export interface SiteLocales {
  /** The default locale's tag ("" while `/meta` loads). */
  default: string;
  /** Every locale, the default first. */
  all: SiteLocale[];
  /** More than one locale: the locale controls show. */
  multilingual: boolean;
  /** The label of `tag`, or the tag itself. */
  label: (tag: string) => string;
}

/** The site's locales from `/meta` (one locale, no controls, until it answers). */
export function useSiteLocales(): SiteLocales {
  const meta = useQuery(metaQuery);
  return siteLocales(meta.data?.locales ?? [], meta.data?.default_locale ?? "");
}

export function siteLocales(all: SiteLocale[], fallback: string): SiteLocales {
  const byTag = new Map(all.map((l) => [l.tag, l.label]));
  return {
    default: all[0]?.tag ?? fallback,
    all,
    multilingual: all.length > 1,
    label: (tag) => byTag.get(tag) ?? tag,
  };
}

/**
 * `tag` in core's spelling (`ferrite_domain::locale::parse_tag`): a language of 2–3
 * letters, then an optional script of 4 letters and an optional region of 2 letters or
 * 3 digits, `-` or `_` between them (`pt_br` → `pt-BR`, `zh-hant-tw` → `zh-Hant-TW`);
 * `null` when core would refuse it.
 */
export function canonicalTag(tag: string): string | null {
  const m = /^([A-Za-z]{2,3})(?:[-_]([A-Za-z]{4}))?(?:[-_]([A-Za-z]{2}|[0-9]{3}))?$/.exec(
    tag.trim(),
  );
  if (!m) return null;
  const [, lang = "", script, region] = m;
  let out = lang.toLowerCase();
  if (script) out += `-${script[0]?.toUpperCase() ?? ""}${script.slice(1).toLowerCase()}`;
  if (region) out += `-${region.toUpperCase()}`;
  return out;
}
