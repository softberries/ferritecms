-- ferrite:phase expand
-- The OAuth 2.1 authorization server's records (design 12 §3.2, §4). `oauth_clients`: a
-- client known by a Client ID Metadata Document URL or by dynamic registration (text id,
-- redirect URIs as JSON). `oauth_requests`: a validated authorization request waiting for
-- consent. `oauth_codes`: single-use authorization codes, stored as SHA-256 hex; `grant_id`
-- (no foreign key: it is stamped when the code is claimed, before the grant row exists)
-- names the grant a used code made, so a replay revokes it. `oauth_grants`: one row per
-- user authorisation, with the current and the previous refresh token hash (replay
-- detection). `api_tokens.oauth_grant_id`: the access tokens of a grant; deleting the
-- grant deletes them.

CREATE TABLE oauth_clients (
    id           TEXT CONSTRAINT oauth_clients_pkey PRIMARY KEY NOT NULL,
    kind         TEXT NOT NULL CHECK (kind IN ('metadata', 'registered')),
    name         TEXT NOT NULL,
    redirect_uris TEXT NOT NULL CHECK (json_valid(redirect_uris)),
    client_uri   TEXT,
    logo_uri     TEXT,
    created_at   INTEGER NOT NULL,
    fetched_at   INTEGER,
    last_used_at INTEGER
);

CREATE TABLE oauth_requests (
    id             BLOB CONSTRAINT oauth_requests_pkey PRIMARY KEY NOT NULL,
    client_id      TEXT NOT NULL CONSTRAINT oauth_requests_client_id_fk REFERENCES oauth_clients (id) ON DELETE CASCADE,
    redirect_uri   TEXT NOT NULL,
    scopes         TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(scopes)),
    state          TEXT,
    code_challenge TEXT NOT NULL,
    resource       TEXT,
    created_at     INTEGER NOT NULL,
    expires_at     INTEGER NOT NULL
);
CREATE INDEX oauth_requests_client_idx ON oauth_requests (client_id);
CREATE INDEX oauth_requests_expires_idx ON oauth_requests (expires_at);

CREATE TABLE oauth_codes (
    code_hash      TEXT CONSTRAINT oauth_codes_pkey PRIMARY KEY NOT NULL,
    client_id      TEXT NOT NULL CONSTRAINT oauth_codes_client_id_fk REFERENCES oauth_clients (id) ON DELETE CASCADE,
    user_id        BLOB NOT NULL CONSTRAINT oauth_codes_user_id_fk REFERENCES users (id) ON DELETE CASCADE,
    redirect_uri   TEXT NOT NULL,
    scopes         TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(scopes)),
    code_challenge TEXT NOT NULL,
    resource       TEXT,
    created_at     INTEGER NOT NULL,
    expires_at     INTEGER NOT NULL,
    grant_id       BLOB
);
CREATE INDEX oauth_codes_client_idx ON oauth_codes (client_id);
CREATE INDEX oauth_codes_user_idx ON oauth_codes (user_id);
CREATE INDEX oauth_codes_expires_idx ON oauth_codes (expires_at);

CREATE TABLE oauth_grants (
    id            BLOB CONSTRAINT oauth_grants_pkey PRIMARY KEY NOT NULL,
    client_id     TEXT NOT NULL CONSTRAINT oauth_grants_client_id_fk REFERENCES oauth_clients (id) ON DELETE CASCADE,
    user_id       BLOB NOT NULL CONSTRAINT oauth_grants_user_id_fk REFERENCES users (id) ON DELETE CASCADE,
    scopes        TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(scopes)),
    refresh_hash  TEXT NOT NULL,
    previous_hash TEXT,
    created_at    INTEGER NOT NULL,
    last_used_at  INTEGER NOT NULL,
    expires_at    INTEGER NOT NULL
);
CREATE UNIQUE INDEX oauth_grants_refresh_hash_uq ON oauth_grants (refresh_hash);
CREATE INDEX oauth_grants_previous_hash_idx ON oauth_grants (previous_hash) WHERE previous_hash IS NOT NULL;
CREATE INDEX oauth_grants_client_idx ON oauth_grants (client_id);
CREATE INDEX oauth_grants_user_idx ON oauth_grants (user_id, created_at);
CREATE INDEX oauth_grants_created_idx ON oauth_grants (created_at, id);
CREATE INDEX oauth_grants_expires_idx ON oauth_grants (expires_at);

-- SQLite adds a column that carries a foreign key when its default is NULL.
ALTER TABLE api_tokens ADD COLUMN oauth_grant_id BLOB
    CONSTRAINT api_tokens_oauth_grant_fk REFERENCES oauth_grants (id) ON DELETE CASCADE;
CREATE INDEX api_tokens_oauth_grant_idx ON api_tokens (oauth_grant_id);
