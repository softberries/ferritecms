-- ferrite:lossy-down
-- Reverse of 0011_artifact_gc.up.sql. Lossy (declared in the up header, where the runner
-- reads it): every orphan mark; the next GC pass marks again.
DROP INDEX artifacts_orphaned_at_idx;
ALTER TABLE artifacts DROP COLUMN orphaned_at;
