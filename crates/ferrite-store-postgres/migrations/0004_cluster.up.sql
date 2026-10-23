-- ferrite:phase expand
-- ferrite:lossy-down   -- the down removes the job queue (queued work is lost) and the node registry
-- Cluster coordination tables (design 04 §6, 02 §4 "Cluster", rulings P5–P8, P27).
-- `jobs`: the leased job queue. Statuses are queued/running/done/dead (no `failed`, P8);
-- `unique_key` NULLs are distinct, so only keyed jobs dedupe. `locked_by` is a node id
-- (UUID text). `nodes`: the node registry (repository in M1.5 task 3). Text keys are
-- COLLATE "C" (R8).

CREATE TABLE jobs (
    id           uuid CONSTRAINT jobs_pkey PRIMARY KEY,
    kind         text COLLATE "C" NOT NULL,
    payload      jsonb NOT NULL,
    unique_key   text COLLATE "C",
    run_at       timestamptz NOT NULL,
    priority     integer NOT NULL DEFAULT 0,
    status       text COLLATE "C" NOT NULL CHECK (status IN ('queued', 'running', 'done', 'dead')),
    attempts     integer NOT NULL DEFAULT 0,
    max_attempts integer NOT NULL DEFAULT 5 CHECK (max_attempts >= 1),
    locked_by    text COLLATE "C",
    locked_until timestamptz,
    last_error   text,
    created_at   timestamptz NOT NULL,
    finished_at  timestamptz
);
CREATE UNIQUE INDEX jobs_unique_key_uq ON jobs (unique_key);
CREATE INDEX jobs_claim_idx ON jobs (status, run_at) WHERE status IN ('queued', 'running');
CREATE INDEX jobs_finished_idx ON jobs (finished_at);

CREATE TABLE nodes (
    id             uuid CONSTRAINT nodes_pkey PRIMARY KEY,
    name           text COLLATE "C" NOT NULL,
    version        text NOT NULL,
    schema_version bigint NOT NULL,
    started_at     timestamptz NOT NULL,
    heartbeat_at   timestamptz NOT NULL,
    meta           jsonb NOT NULL DEFAULT '{}'
);
CREATE INDEX nodes_heartbeat_idx ON nodes (heartbeat_at);
