-- ferrite:phase expand
-- ferrite:lossy-down   -- the down removes sessions, API tokens, rate-limit counters and the audit log
-- Authentication tables (design 07 §2, 02 §4 "Users & auth", rulings P2, P5, P7, P22).
-- `sessions`: one row per login, keyed by the SHA-256 hex of the cookie token. `api_tokens`:
-- personal bearer tokens, stored as the SHA-256 hex of the bearer value. `rate_limits`:
-- fixed-window counters, one row per bucket (P5). `audit_log`: admin actions; the actor
-- becomes NULL when the user is deleted. `users.totp_last_step`: the last accepted TOTP
-- step, so a code cannot be replayed (P9).

CREATE TABLE sessions (
    id_hash      TEXT CONSTRAINT sessions_pkey PRIMARY KEY NOT NULL,
    user_id      BLOB NOT NULL CONSTRAINT sessions_user_id_fk REFERENCES users (id) ON DELETE CASCADE,
    csrf_secret  TEXT NOT NULL,
    created_at   INTEGER NOT NULL,
    last_seen_at INTEGER NOT NULL,
    expires_at   INTEGER NOT NULL,
    ip_hash      TEXT,
    user_agent   TEXT
);
CREATE INDEX sessions_user_idx ON sessions (user_id);
CREATE INDEX sessions_expires_idx ON sessions (expires_at);
CREATE INDEX sessions_last_seen_idx ON sessions (last_seen_at);

CREATE TABLE api_tokens (
    id           BLOB CONSTRAINT api_tokens_pkey PRIMARY KEY NOT NULL,
    user_id      BLOB NOT NULL CONSTRAINT api_tokens_user_id_fk REFERENCES users (id) ON DELETE CASCADE,
    name         TEXT NOT NULL,
    token_hash   TEXT NOT NULL,
    scopes       TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(scopes)),
    created_at   INTEGER NOT NULL,
    last_used_at INTEGER,
    expires_at   INTEGER
);
CREATE UNIQUE INDEX api_tokens_hash_uq ON api_tokens (token_hash);
CREATE INDEX api_tokens_user_idx ON api_tokens (user_id);

CREATE TABLE rate_limits (
    bucket       TEXT CONSTRAINT rate_limits_pkey PRIMARY KEY NOT NULL,
    window_start INTEGER NOT NULL,
    count        INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX rate_limits_window_idx ON rate_limits (window_start);

CREATE TABLE audit_log (
    id          BLOB CONSTRAINT audit_log_pkey PRIMARY KEY NOT NULL,
    at          INTEGER NOT NULL,
    actor_id    BLOB CONSTRAINT audit_log_actor_id_fk REFERENCES users (id) ON DELETE SET NULL,
    action      TEXT NOT NULL,
    target_kind TEXT NOT NULL,
    target_id   TEXT,
    details     TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(details)),
    node        TEXT
);
CREATE INDEX audit_log_at_idx ON audit_log (at, id);
-- Deleting a user sets actor_id NULL here (and reassigns) without a scan.
CREATE INDEX audit_log_actor_idx ON audit_log (actor_id);
-- Reassigning a deleted author's revisions, and the users FK check on delete, without a
-- scan (content_items_author_idx exists since 0001).
CREATE INDEX revisions_author_idx ON revisions (author_id);

ALTER TABLE users ADD COLUMN totp_last_step INTEGER;
