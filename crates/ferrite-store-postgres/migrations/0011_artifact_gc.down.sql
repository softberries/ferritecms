-- Reverse of 0011_artifact_gc.up.sql. Not lossy: the marks only delay deletion, and the
-- next GC pass marks again.
DROP INDEX artifacts_orphaned_at_idx;
ALTER TABLE artifacts DROP COLUMN orphaned_at;
