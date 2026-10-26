-- ferrite:phase expand
-- ferrite:lossy-down   -- the down drops the SEO member of every autosave
-- The SEO title and description of the editor's working copy (M2b-2 Task 1, P28): the
-- `seo` patch of the autosave slot as the editor sent it, a JSON object with optional
-- `title` and `description` members (each a string or null). NULL = unchanged, like
-- `slug` (F11). The item's own SEO lives in `content_meta` (`seo.title`,
-- `seo.description`).

ALTER TABLE autosaves ADD COLUMN seo jsonb NULL;
