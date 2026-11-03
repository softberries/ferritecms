-- ferrite:phase expand
-- AI change sets (design 17 §5). `change_sets`: one working session of one MCP agent
-- (an OAuth grant or a personal token; `agent_id` has no foreign key: grants and tokens
-- are deleted while their history stays). `open_key` ('oauth:<id>' / 'token:<id>') is set
-- while the set takes new changes; UNIQUE so concurrent writers of one agent meet in one
-- row. `change_set_changes`: one object touched by one write, with zstd-compressed
-- canonical JSON before/after images and their SHA-256 hashes (hash NULL = absent; hash
-- without bytes = too large to keep). Text keys are COLLATE "C" (R8).
CREATE TABLE change_sets (
    id          uuid CONSTRAINT change_sets_pkey PRIMARY KEY,
    user_id     uuid CONSTRAINT change_sets_user_id_fk REFERENCES users (id) ON DELETE SET NULL,
    agent_kind  text NOT NULL CHECK (agent_kind IN ('oauth', 'token')),
    agent_id    uuid NOT NULL,
    client      text NOT NULL,
    open_key    text COLLATE "C" CONSTRAINT change_sets_open_key_uq UNIQUE,
    started_at  timestamptz NOT NULL,
    last_at     timestamptz NOT NULL,
    changes     integer NOT NULL DEFAULT 0,
    reverted_at timestamptz,
    reverted_by uuid CONSTRAINT change_sets_reverted_by_fk REFERENCES users (id) ON DELETE SET NULL
);
CREATE INDEX change_sets_user_idx ON change_sets (user_id, last_at);
CREATE INDEX change_sets_agent_idx ON change_sets (agent_id, last_at);
CREATE INDEX change_sets_last_idx ON change_sets (last_at);
CREATE INDEX change_sets_reverted_by_idx ON change_sets (reverted_by);

CREATE TABLE change_set_changes (
    set_id        uuid NOT NULL CONSTRAINT change_set_changes_set_id_fk REFERENCES change_sets (id) ON DELETE CASCADE,
    seq           integer NOT NULL,
    at            timestamptz NOT NULL,
    op            text COLLATE "C" NOT NULL,
    object_kind   text COLLATE "C" NOT NULL,
    object_id     text COLLATE "C" NOT NULL,
    label         text NOT NULL DEFAULT '',
    before_image  bytea,
    before_hash   text,
    after_image   bytea,
    after_hash    text,
    reverted_at   timestamptz,
    reverted_by   uuid CONSTRAINT change_set_changes_reverted_by_fk REFERENCES users (id) ON DELETE SET NULL,
    revert_forced boolean NOT NULL DEFAULT false,
    CONSTRAINT change_set_changes_pkey PRIMARY KEY (set_id, seq)
);
CREATE INDEX change_set_changes_object_idx ON change_set_changes (object_kind, object_id);
CREATE INDEX change_set_changes_reverted_by_idx ON change_set_changes (reverted_by);
