-- ferrite:phase expand
-- ferrite:lossy-down   -- the down removes idempotency keys and the sanitised-SVG marker
-- Media uploads and comments (M2a-2b P2, P7, P10, P11; F4, F19, F26).
-- `idempotency_keys`: one row per (user, SHA-256 hex of the `Idempotency-Key`); a NULL
-- `status_code` is a pending request, a finished one stores its status and response JSON
-- (both or neither). `media.sanitized`: the stored bytes came out of the SVG sanitiser,
-- so the file may be served inline. Comment indexes: the moderation queue (status, or
-- all, newest first), an item's approved thread (oldest first; also the queue filtered
-- by item and status), the `known` checks by e-mail (anonymous) and by user (which also
-- serves the users FK on delete). Job indexes: the admin jobs list by status or all.

CREATE TABLE idempotency_keys (
    user_id     BLOB NOT NULL CONSTRAINT idempotency_keys_user_id_fk REFERENCES users (id) ON DELETE CASCADE,
    key_hash    TEXT NOT NULL,
    method      TEXT NOT NULL,
    path        TEXT NOT NULL,
    status_code INTEGER,
    response    TEXT CHECK (response IS NULL OR json_valid(response)),
    created_at  INTEGER NOT NULL,
    CONSTRAINT idempotency_keys_pkey PRIMARY KEY (user_id, key_hash),
    CHECK ((status_code IS NULL) = (response IS NULL))
);
CREATE INDEX idempotency_keys_created_idx ON idempotency_keys (created_at);

ALTER TABLE media ADD COLUMN sanitized INTEGER NOT NULL DEFAULT 0 CHECK (sanitized IN (0, 1));

CREATE INDEX comments_status_created_idx ON comments (status, created_at, id);
CREATE INDEX comments_item_status_idx ON comments (item_id, status, created_at, id);
CREATE INDEX comments_created_idx ON comments (created_at, id);
CREATE INDEX comments_author_email_idx ON comments (author_email, status);
CREATE INDEX comments_user_idx ON comments (user_id, status);

CREATE INDEX jobs_status_created_idx ON jobs (status, created_at, id);
CREATE INDEX jobs_created_idx ON jobs (created_at, id);
