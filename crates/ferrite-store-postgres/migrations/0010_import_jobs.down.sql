-- ferrite:lossy-down
-- Reverse of 0010_import_jobs.up.sql. Lossy (declared in the up header, where the runner
-- reads it): the runs' job, step and report hash, and every `import` artifact row (their
-- files stay in the media store until it is cleaned).
DROP INDEX import_runs_job_idx;
ALTER TABLE import_runs
    DROP COLUMN report_hash,
    DROP COLUMN step,
    DROP COLUMN job_id;

DELETE FROM artifacts WHERE kind = 'import';
ALTER TABLE artifacts
    DROP CONSTRAINT artifacts_kind_check,
    ADD CONSTRAINT artifacts_kind_check
        CHECK (kind IN ('media', 'variant', 'plugin', 'theme', 'export'));
