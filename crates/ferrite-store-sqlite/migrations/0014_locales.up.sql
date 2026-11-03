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
-- start. The two slug unique indexes are widened by `locale` under their own names (the
-- `widens_unique` expand rule): every pair of rows the old indexes allowed is still
-- allowed.

ALTER TABLE content_items ADD COLUMN locale TEXT NOT NULL DEFAULT '';
ALTER TABLE content_items ADD COLUMN translation_group BLOB;
UPDATE content_items SET translation_group = id, locale = coalesce(
    (SELECT json_extract(value, '$') FROM options
     WHERE key = 'site.locale' AND json_type(value) = 'text'), '');
UPDATE content_items SET locale = replace(locale, '_', '-');
UPDATE content_items SET locale = substr(locale, 1, instr(locale, '.') - 1)
    WHERE instr(locale, '.') > 0;
UPDATE content_items SET locale = substr(locale, 1, instr(locale, '@') - 1)
    WHERE instr(locale, '@') > 0;
UPDATE content_items SET locale = 'en' WHERE trim(locale, '-') = '';

ALTER TABLE terms ADD COLUMN locale TEXT NOT NULL DEFAULT '';
ALTER TABLE terms ADD COLUMN translation_group BLOB;
UPDATE terms SET translation_group = id, locale = coalesce(
    (SELECT json_extract(value, '$') FROM options
     WHERE key = 'site.locale' AND json_type(value) = 'text'), '');
UPDATE terms SET locale = replace(locale, '_', '-');
UPDATE terms SET locale = substr(locale, 1, instr(locale, '.') - 1)
    WHERE instr(locale, '.') > 0;
UPDATE terms SET locale = substr(locale, 1, instr(locale, '@') - 1)
    WHERE instr(locale, '@') > 0;
UPDATE terms SET locale = 'en' WHERE trim(locale, '-') = '';

ALTER TABLE menus ADD COLUMN locale TEXT NOT NULL DEFAULT '';
ALTER TABLE menus ADD COLUMN translation_of TEXT;
UPDATE menus SET locale = coalesce(
    (SELECT json_extract(value, '$') FROM options
     WHERE key = 'site.locale' AND json_type(value) = 'text'), '');
UPDATE menus SET locale = replace(locale, '_', '-');
UPDATE menus SET locale = substr(locale, 1, instr(locale, '.') - 1)
    WHERE instr(locale, '.') > 0;
UPDATE menus SET locale = substr(locale, 1, instr(locale, '@') - 1)
    WHERE instr(locale, '@') > 0;
UPDATE menus SET locale = 'en' WHERE trim(locale, '-') = '';

DROP INDEX content_items_slug_uq;
CREATE UNIQUE INDEX content_items_slug_uq
    ON content_items (type, locale, coalesce(parent_id, x''), slug);
DROP INDEX terms_taxonomy_slug_uq;
CREATE UNIQUE INDEX terms_taxonomy_slug_uq ON terms (taxonomy, locale, slug);
CREATE UNIQUE INDEX content_items_translation_uq
    ON content_items (translation_group, locale);
CREATE INDEX content_items_locale_list_idx
    ON content_items (type, locale, status, published_at DESC, id DESC);
CREATE UNIQUE INDEX terms_translation_uq ON terms (translation_group, locale);
CREATE UNIQUE INDEX menus_translation_uq ON menus (translation_of, locale);
