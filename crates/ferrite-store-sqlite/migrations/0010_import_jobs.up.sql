-- ferrite:phase contract
-- ferrite:lossy-down   -- the down drops every `import` artifact row and the run columns
-- Import jobs (M3-2 P23, P28). `artifacts.kind` takes `import`: the uploaded WXR and
-- uploads zip are artifacts until the run is done. `import_runs` gains `job_id` (the
-- `import.wordpress` job that started the run, no foreign key since jobs are pruned),
-- `step` (the step the run is in) and `report_hash` (the report artifact).
--
-- Contract because of the lint only: SQLite cannot alter a CHECK, so `artifacts` is
-- rebuilt (copy, DROP TABLE, create, copy back). The rebuild only widens the CHECK, so
-- an older binary keeps working against it and no min-binary is set. `media`, `plugins`
-- and `themes` reference `artifacts (hash)` without ON DELETE actions; the foreign keys
-- cannot be switched off inside the migration's transaction, so they are deferred: the
-- DROP's implicit delete counts the orphaned child rows and the copy back clears them
-- before the commit. Renaming a new table into place is avoided because it would
-- rewrite the children's REFERENCES clauses to the old name.
PRAGMA defer_foreign_keys = ON;
CREATE TABLE artifacts_0010 AS SELECT hash, kind, bytes, mime, created_at FROM artifacts;
DROP TABLE artifacts;
CREATE TABLE artifacts (
    hash       TEXT CONSTRAINT artifacts_pkey PRIMARY KEY NOT NULL,
    kind       TEXT NOT NULL CHECK (kind IN ('media', 'variant', 'plugin', 'theme', 'export', 'import')),
    bytes      INTEGER NOT NULL,
    mime       TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
INSERT INTO artifacts (hash, kind, bytes, mime, created_at)
SELECT hash, kind, bytes, mime, created_at FROM artifacts_0010;
DROP TABLE artifacts_0010;

ALTER TABLE import_runs ADD COLUMN job_id BLOB;
ALTER TABLE import_runs ADD COLUMN step TEXT;
ALTER TABLE import_runs ADD COLUMN report_hash TEXT;
CREATE INDEX import_runs_job_idx ON import_runs (job_id);
