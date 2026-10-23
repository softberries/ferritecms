-- ferrite:phase expand
-- ferrite:lossy-down   -- the down removes the job queue (queued work is lost) and the node registry
-- Cluster coordination tables (design 04 §6, 02 §4 "Cluster", rulings P5–P8, P27).
-- `jobs`: the leased job queue. Statuses are queued/running/done/dead (no `failed`, P8);
-- `unique_key` NULLs are distinct, so only keyed jobs dedupe. `locked_by` is a node id
-- (UUID text). `nodes`: the node registry (repository in M1.5 task 3).

CREATE TABLE jobs (
    id           BLOB CONSTRAINT jobs_pkey PRIMARY KEY NOT NULL,
    kind         TEXT NOT NULL,
    payload      TEXT NOT NULL CHECK (json_valid(payload)),
    unique_key   TEXT,
    run_at       INTEGER NOT NULL,
    priority     INTEGER NOT NULL DEFAULT 0,
    status       TEXT NOT NULL CHECK (status IN ('queued', 'running', 'done', 'dead')),
    attempts     INTEGER NOT NULL DEFAULT 0,
    max_attempts INTEGER NOT NULL DEFAULT 5 CHECK (max_attempts >= 1),
    locked_by    TEXT,
    locked_until INTEGER,
    last_error   TEXT,
    created_at   INTEGER NOT NULL,
    finished_at  INTEGER
);
CREATE UNIQUE INDEX jobs_unique_key_uq ON jobs (unique_key);
CREATE INDEX jobs_claim_idx ON jobs (status, run_at);
CREATE INDEX jobs_finished_idx ON jobs (finished_at);

CREATE TABLE nodes (
    id             BLOB CONSTRAINT nodes_pkey PRIMARY KEY NOT NULL,
    name           TEXT NOT NULL,
    version        TEXT NOT NULL,
    schema_version INTEGER NOT NULL,
    started_at     INTEGER NOT NULL,
    heartbeat_at   INTEGER NOT NULL,
    meta           TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(meta))
);
CREATE INDEX nodes_heartbeat_idx ON nodes (heartbeat_at);
