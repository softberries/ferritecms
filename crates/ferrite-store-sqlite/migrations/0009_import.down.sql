-- ferrite:lossy-down
-- Reverse of 0009_import.up.sql. Lossy (declared in the up header, where the runner
-- reads it): every import run and the source-id map are lost.
DROP TABLE import_map;
DROP INDEX import_runs_started_by_idx;
DROP INDEX import_runs_started_idx;
DROP INDEX import_runs_running_uq;
DROP TABLE import_runs;
