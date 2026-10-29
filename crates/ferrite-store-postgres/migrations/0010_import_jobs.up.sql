-- ferrite:phase expand
-- ferrite:lossy-down   -- the down drops every `import` artifact row and the run columns
-- Import jobs (M3-2 P23, P28). `artifacts.kind` takes `import`: the uploaded WXR and
-- uploads zip are artifacts until the run is done. `import_runs` gains `job_id` (the
-- `import.wordpress` job that started the run, no foreign key since jobs are pruned),
-- `step` (the step the run is in) and `report_hash` (the report artifact).
-- The CHECK is replaced in one ALTER TABLE: it only widens, so an older binary keeps
-- working against it (the expand lint accepts a constraint dropped and re-added under
-- the same name in one statement). SQLite needs a table rebuild for the same change,
-- which makes its 0010 a contract migration.
ALTER TABLE artifacts
    DROP CONSTRAINT artifacts_kind_check,
    ADD CONSTRAINT artifacts_kind_check
        CHECK (kind IN ('media', 'variant', 'plugin', 'theme', 'export', 'import'));

ALTER TABLE import_runs
    ADD COLUMN job_id uuid,
    ADD COLUMN step text COLLATE "C",
    ADD COLUMN report_hash text COLLATE "C";
CREATE INDEX import_runs_job_idx ON import_runs (job_id);
