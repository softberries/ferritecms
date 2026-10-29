-- ferrite:phase expand
-- ferrite:lossy-down   -- the down drops every orphan mark
-- Artifact garbage collection (M4-1 Task 1, P11). `orphaned_at` is when the GC first saw
-- the artifact unreferenced (NULL = referenced or not yet seen); a row is deleted only
-- when it has been marked for longer than the grace period. The partial index serves
-- "marked before T".

ALTER TABLE artifacts ADD COLUMN orphaned_at INTEGER;
CREATE INDEX artifacts_orphaned_at_idx ON artifacts (orphaned_at) WHERE orphaned_at IS NOT NULL;
