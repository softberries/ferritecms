-- ferrite:phase expand
-- ferrite:lossy-down   -- the down removes sessions, API tokens, rate-limit counters and the audit log
-- Authentication tables (design 07 §2, 02 §4 "Users & auth", rulings P2, P5, P7, P22).
-- `sessions`: one row per login, keyed by the SHA-256 hex of the cookie token. `api_tokens`:
-- personal bearer tokens, stored as the SHA-256 hex of the bearer value. `rate_limits`:
-- fixed-window counters, one row per bucket (P5). `audit_log`: admin actions; the actor
-- becomes NULL when the user is deleted. `users.totp_last_step`: the last accepted TOTP
-- step, so a code cannot be replayed (P9). Text keys are COLLATE "C" (R8).

CREATE TABLE sessions (
    id_hash      text COLLATE "C" CONSTRAINT sessions_pkey PRIMARY KEY,
    user_id      uuid NOT NULL CONSTRAINT sessions_user_id_fk REFERENCES users (id) ON DELETE CASCADE,
    csrf_secret  text NOT NULL,
    created_at   timestamptz NOT NULL,
    last_seen_at timestamptz NOT NULL,
    expires_at   timestamptz NOT NULL,
    ip_hash      text COLLATE "C",
    user_agent   text
);
CREATE INDEX sessions_user_idx ON sessions (user_id);
CREATE INDEX sessions_expires_idx ON sessions (expires_at);
CREATE INDEX sessions_last_seen_idx ON sessions (last_seen_at);

CREATE TABLE api_tokens (
    id           uuid CONSTRAINT api_tokens_pkey PRIMARY KEY,
    user_id      uuid NOT NULL CONSTRAINT api_tokens_user_id_fk REFERENCES users (id) ON DELETE CASCADE,
    name         text NOT NULL,
    token_hash   text COLLATE "C" NOT NULL,
    scopes       jsonb NOT NULL DEFAULT '[]',
    created_at   timestamptz NOT NULL,
    last_used_at timestamptz,
    expires_at   timestamptz
);
CREATE UNIQUE INDEX api_tokens_hash_uq ON api_tokens (token_hash);
CREATE INDEX api_tokens_user_idx ON api_tokens (user_id);

CREATE TABLE rate_limits (
    bucket       text COLLATE "C" CONSTRAINT rate_limits_pkey PRIMARY KEY,
    window_start timestamptz NOT NULL,
    count        integer NOT NULL DEFAULT 0
);
CREATE INDEX rate_limits_window_idx ON rate_limits (window_start);

CREATE TABLE audit_log (
    id          uuid CONSTRAINT audit_log_pkey PRIMARY KEY,
    at          timestamptz NOT NULL,
    actor_id    uuid CONSTRAINT audit_log_actor_id_fk REFERENCES users (id) ON DELETE SET NULL,
    action      text COLLATE "C" NOT NULL,
    target_kind text COLLATE "C" NOT NULL,
    target_id   text COLLATE "C",
    details     jsonb NOT NULL DEFAULT '{}',
    node        text COLLATE "C"
);
CREATE INDEX audit_log_at_idx ON audit_log (at, id);
-- Deleting a user sets actor_id NULL here (and reassigns) without a scan.
CREATE INDEX audit_log_actor_idx ON audit_log (actor_id);

ALTER TABLE users ADD COLUMN totp_last_step bigint;
