-- ferrite:lossy-down
-- Reverse of 0010_import_jobs.up.sql. Lossy (declared in the up header, where the runner
-- reads it): the runs' job, step and report hash, and every `import` artifact row (their
-- files stay in the media store until it is cleaned).
DROP INDEX import_runs_job_idx;
ALTER TABLE import_runs DROP COLUMN report_hash;
ALTER TABLE import_runs DROP COLUMN step;
ALTER TABLE import_runs DROP COLUMN job_id;

-- The same rebuild as the up, back to the 0001 CHECK.
PRAGMA defer_foreign_keys = ON;
CREATE TABLE artifacts_0010 AS
SELECT hash, kind, bytes, mime, created_at FROM artifacts WHERE kind <> 'import';
DROP TABLE artifacts;
CREATE TABLE artifacts (
    hash       TEXT CONSTRAINT artifacts_pkey PRIMARY KEY NOT NULL,
    kind       TEXT NOT NULL CHECK (kind IN ('media', 'variant', 'plugin', 'theme', 'export')),
    bytes      INTEGER NOT NULL,
    mime       TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
INSERT INTO artifacts (hash, kind, bytes, mime, created_at)
SELECT hash, kind, bytes, mime, created_at FROM artifacts_0010;
DROP TABLE artifacts_0010;
