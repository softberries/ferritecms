-- ferrite:lossy-down
-- Reverse of 0007_autosaves.up.sql. Lossy (declared in the up header, where the runner
-- reads it): every unsaved autosave is lost.
DROP INDEX autosaves_user_idx;
DROP TABLE autosaves;
