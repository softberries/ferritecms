-- Reverse of 0014_locales.up.sql. Lossy (declared in the up header): the content items,
-- terms and menus whose locale is not the default (the first tag of `site.locales`) are
-- deleted, with what cascades from an item (meta, revisions, term links, comments,
-- autosaves, search documents, media rows) or a menu (its items), and the options
-- `site.locales` and `site.locale_texts`. Rows with the blank locale an older binary
-- writes count as the default. Media items are never deleted (the library is shared by
-- every locale, whatever locale its rows carry). A surviving item or term whose parent is
-- deleted becomes top level.

UPDATE content_items SET parent_id = NULL
WHERE parent_id IN (
    SELECT id FROM content_items
    WHERE type <> 'media' AND locale <> '' AND locale <> (
        SELECT json_extract(value, '$[0].tag') FROM options WHERE key = 'site.locales'))
  AND NOT (locale <> '' AND locale <> (
        SELECT json_extract(value, '$[0].tag') FROM options WHERE key = 'site.locales'));
DELETE FROM content_items
WHERE type <> 'media' AND locale <> '' AND locale <> (
    SELECT json_extract(value, '$[0].tag') FROM options WHERE key = 'site.locales');

UPDATE terms SET parent_id = NULL
WHERE parent_id IN (
    SELECT id FROM terms
    WHERE locale <> '' AND locale <> (
        SELECT json_extract(value, '$[0].tag') FROM options WHERE key = 'site.locales'))
  AND NOT (locale <> '' AND locale <> (
        SELECT json_extract(value, '$[0].tag') FROM options WHERE key = 'site.locales'));
DELETE FROM terms
WHERE locale <> '' AND locale <> (
    SELECT json_extract(value, '$[0].tag') FROM options WHERE key = 'site.locales');

DELETE FROM menus
WHERE locale <> '' AND locale <> (
    SELECT json_extract(value, '$[0].tag') FROM options WHERE key = 'site.locales');

DELETE FROM options WHERE key IN ('site.locales', 'site.locale_texts');

DROP INDEX menus_translation_uq;
DROP INDEX terms_translation_uq;
DROP INDEX content_items_locale_list_idx;
DROP INDEX content_items_translation_uq;
DROP INDEX terms_taxonomy_slug_uq;
CREATE UNIQUE INDEX terms_taxonomy_slug_uq ON terms (taxonomy, slug);
DROP INDEX content_items_slug_uq;
CREATE UNIQUE INDEX content_items_slug_uq ON content_items (type, coalesce(parent_id, x''), slug);

ALTER TABLE menus DROP COLUMN translation_of;
ALTER TABLE menus DROP COLUMN locale;
ALTER TABLE terms DROP COLUMN translation_group;
ALTER TABLE terms DROP COLUMN locale;
ALTER TABLE content_items DROP COLUMN translation_group;
ALTER TABLE content_items DROP COLUMN locale;
