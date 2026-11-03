-- ferrite:phase expand
-- ferrite:lossy-down
-- Multilingual content (design 13 §3). Every content item, term and menu gets a
-- `locale` (a BCP 47 tag; '' only for rows an older binary writes, which `settle`
-- relabels at the next start) and items and terms a `translation_group` (the id of the
-- group's original; NULL only for rows an older binary writes, filled by `settle`). Menus
-- get `translation_of`, the name of the default-locale menu they translate.
--
-- The backfill's locale is a guess from the `site.locale` option (`pt_BR.UTF-8` → `pt-BR`),
-- else `en`; `ferrite_core::locales::settle` corrects it from the config at the first
-- start. The slug constraint and the term slug index are widened by `locale` under their
-- own names (the `widens_unique` expand rule): every pair of rows the old ones allowed is
-- still allowed.

ALTER TABLE content_items
    ADD COLUMN locale text COLLATE "C" NOT NULL DEFAULT '',
    ADD COLUMN translation_group uuid;
ALTER TABLE terms
    ADD COLUMN locale text COLLATE "C" NOT NULL DEFAULT '',
    ADD COLUMN translation_group uuid;
ALTER TABLE menus
    ADD COLUMN locale text COLLATE "C" NOT NULL DEFAULT '',
    ADD COLUMN translation_of text COLLATE "C";

UPDATE content_items SET translation_group = id, locale = coalesce(nullif(trim(BOTH '-' FROM
    split_part(split_part(replace(
        (SELECT value #>> '{}' FROM options
         WHERE key = 'site.locale' AND jsonb_typeof(value) = 'string'),
        '_', '-'), '.', 1), '@', 1)), ''), 'en');
UPDATE terms SET translation_group = id, locale = coalesce(nullif(trim(BOTH '-' FROM
    split_part(split_part(replace(
        (SELECT value #>> '{}' FROM options
         WHERE key = 'site.locale' AND jsonb_typeof(value) = 'string'),
        '_', '-'), '.', 1), '@', 1)), ''), 'en');
UPDATE menus SET locale = coalesce(nullif(trim(BOTH '-' FROM
    split_part(split_part(replace(
        (SELECT value #>> '{}' FROM options
         WHERE key = 'site.locale' AND jsonb_typeof(value) = 'string'),
        '_', '-'), '.', 1), '@', 1)), ''), 'en');

ALTER TABLE content_items
    DROP CONSTRAINT content_items_slug_uq,
    ADD CONSTRAINT content_items_slug_uq UNIQUE NULLS NOT DISTINCT (type, locale, parent_id, slug);
DROP INDEX terms_taxonomy_slug_uq;
CREATE UNIQUE INDEX terms_taxonomy_slug_uq ON terms (taxonomy, locale, slug);
CREATE UNIQUE INDEX content_items_translation_uq
    ON content_items (translation_group, locale);
CREATE INDEX content_items_locale_list_idx
    ON content_items (type, locale, status, published_at DESC NULLS LAST, id DESC);
CREATE UNIQUE INDEX terms_translation_uq ON terms (translation_group, locale);
CREATE UNIQUE INDEX menus_translation_uq ON menus (translation_of, locale);
