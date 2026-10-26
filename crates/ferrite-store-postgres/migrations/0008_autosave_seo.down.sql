-- ferrite:lossy-down
-- Reverse of 0008_autosave_seo.up.sql. Lossy (declared in the up header, where the
-- runner reads it): every autosave loses its SEO member; the slots themselves stay.
ALTER TABLE autosaves DROP COLUMN seo;
