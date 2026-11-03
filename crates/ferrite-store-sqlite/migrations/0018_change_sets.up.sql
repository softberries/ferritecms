-- ferrite:phase expand
-- AI change sets (design 17 §5). `change_sets`: one working session of one MCP agent
-- (an OAuth grant or a personal token; `agent_id` has no foreign key: grants and tokens
-- are deleted while their history stays). `open_key` ('oauth:<id>' / 'token:<id>') is set
-- while the set takes new changes; UNIQUE so concurrent writers of one agent meet in one
-- row. `change_set_changes`: one object touched by one write, with zstd-compressed
-- canonical JSON before/after images and their SHA-256 hashes (hash NULL = absent; hash
-- without bytes = too large to keep).
CREATE TABLE change_sets (
    id          BLOB CONSTRAINT change_sets_pkey PRIMARY KEY NOT NULL,
    user_id     BLOB CONSTRAINT change_sets_user_id_fk REFERENCES users (id) ON DELETE SET NULL,
    agent_kind  TEXT NOT NULL CHECK (agent_kind IN ('oauth', 'token')),
    agent_id    BLOB NOT NULL,
    client      TEXT NOT NULL,
    open_key    TEXT CONSTRAINT change_sets_open_key_uq UNIQUE,
    started_at  INTEGER NOT NULL,
    last_at     INTEGER NOT NULL,
    changes     INTEGER NOT NULL DEFAULT 0,
    reverted_at INTEGER,
    reverted_by BLOB CONSTRAINT change_sets_reverted_by_fk REFERENCES users (id) ON DELETE SET NULL
);
CREATE INDEX change_sets_user_idx ON change_sets (user_id, last_at);
CREATE INDEX change_sets_agent_idx ON change_sets (agent_id, last_at);
CREATE INDEX change_sets_last_idx ON change_sets (last_at);
CREATE INDEX change_sets_reverted_by_idx ON change_sets (reverted_by);

CREATE TABLE change_set_changes (
    set_id        BLOB NOT NULL CONSTRAINT change_set_changes_set_id_fk REFERENCES change_sets (id) ON DELETE CASCADE,
    seq           INTEGER NOT NULL,
    at            INTEGER NOT NULL,
    op            TEXT NOT NULL,
    object_kind   TEXT NOT NULL,
    object_id     TEXT NOT NULL,
    label         TEXT NOT NULL DEFAULT '',
    before_image  BLOB,
    before_hash   TEXT,
    after_image   BLOB,
    after_hash    TEXT,
    reverted_at   INTEGER,
    reverted_by   BLOB CONSTRAINT change_set_changes_reverted_by_fk REFERENCES users (id) ON DELETE SET NULL,
    revert_forced INTEGER NOT NULL DEFAULT 0 CHECK (revert_forced IN (0, 1)),
    CONSTRAINT change_set_changes_pkey PRIMARY KEY (set_id, seq)
);
CREATE INDEX change_set_changes_object_idx ON change_set_changes (object_kind, object_id);
CREATE INDEX change_set_changes_reverted_by_idx ON change_set_changes (reverted_by);
