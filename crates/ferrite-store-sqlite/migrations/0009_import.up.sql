-- ferrite:phase expand
-- ferrite:lossy-down   -- the down drops every import run and the whole source-id map
-- WordPress import bookkeeping (M3-1 P3, P5). `import_runs` is one row per run; `token`
-- is the run owner's 128-bit secret as 32 lowercase hex digits, and every batch proves
-- it still holds it with an UPDATE of `heartbeat_at` (the fence). The partial unique
-- index allows at most one `running` run. `started_by` cascades: a deleted user takes
-- their run history with them (a `running` run then fails its next fence).
-- `import_map` maps (source, kind, wp_id) to what the import wrote, so a re-run from a
-- fresh upload updates instead of duplicating; `ferrite_id` has no foreign key (a row
-- may outlive its item and is then treated as "not mapped"). `source` and `hash` use
-- the default BINARY collation (parity with Postgres "C").

CREATE TABLE import_runs (
    id           BLOB NOT NULL CONSTRAINT import_runs_pkey PRIMARY KEY,
    source       TEXT NOT NULL,
    token        TEXT NOT NULL,
    state        TEXT NOT NULL CONSTRAINT import_runs_state_check
                 CHECK (state IN ('running', 'done', 'failed', 'abandoned')),
    started_by   BLOB NOT NULL CONSTRAINT import_runs_started_by_fk REFERENCES users (id) ON DELETE CASCADE,
    started_at   INTEGER NOT NULL,
    heartbeat_at INTEGER NOT NULL,
    finished_at  INTEGER,
    stats        TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(stats)),
    dry_run      INTEGER NOT NULL CHECK (dry_run IN (0, 1))
);
CREATE UNIQUE INDEX import_runs_running_uq ON import_runs (state) WHERE state = 'running';
CREATE INDEX import_runs_started_idx ON import_runs (started_at, id);
-- Serves the users foreign key on delete.
CREATE INDEX import_runs_started_by_idx ON import_runs (started_by);

CREATE TABLE import_map (
    source      TEXT NOT NULL,
    kind        TEXT NOT NULL CONSTRAINT import_map_kind_check
                CHECK (kind IN ('user', 'term', 'media', 'content', 'comment', 'menu', 'option')),
    wp_id       INTEGER NOT NULL,
    ferrite_id  BLOB NOT NULL,
    hash        TEXT NOT NULL,
    version     INTEGER NOT NULL,
    CONSTRAINT import_map_pkey PRIMARY KEY (source, kind, wp_id)
);
