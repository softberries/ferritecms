-- ferrite:phase expand
-- The OAuth 2.1 authorization server's records (design 12 §3.2, §4). `oauth_clients`: a
-- client known by a Client ID Metadata Document URL or by dynamic registration (text id,
-- redirect URIs as jsonb). `oauth_requests`: a validated authorization request waiting for
-- consent. `oauth_codes`: single-use authorization codes, stored as SHA-256 hex; `grant_id`
-- (no foreign key: it is stamped when the code is claimed, before the grant row exists)
-- names the grant a used code made, so a replay revokes it. `oauth_grants`: one row per
-- user authorisation, with the current and the previous refresh token hash (replay
-- detection). `api_tokens.oauth_grant_id`: the access tokens of a grant; deleting the
-- grant deletes them. Text keys are COLLATE "C" (R8).

CREATE TABLE oauth_clients (
    id            text COLLATE "C" CONSTRAINT oauth_clients_pkey PRIMARY KEY,
    kind          text NOT NULL CHECK (kind IN ('metadata', 'registered')),
    name          text NOT NULL,
    redirect_uris jsonb NOT NULL,
    client_uri    text,
    logo_uri      text,
    created_at    timestamptz NOT NULL,
    fetched_at    timestamptz,
    last_used_at  timestamptz
);

CREATE TABLE oauth_requests (
    id             uuid CONSTRAINT oauth_requests_pkey PRIMARY KEY,
    client_id      text COLLATE "C" NOT NULL CONSTRAINT oauth_requests_client_id_fk REFERENCES oauth_clients (id) ON DELETE CASCADE,
    redirect_uri   text NOT NULL,
    scopes         jsonb NOT NULL DEFAULT '[]',
    state          text,
    code_challenge text NOT NULL,
    resource       text,
    created_at     timestamptz NOT NULL,
    expires_at     timestamptz NOT NULL
);
CREATE INDEX oauth_requests_client_idx ON oauth_requests (client_id);
CREATE INDEX oauth_requests_expires_idx ON oauth_requests (expires_at);

CREATE TABLE oauth_codes (
    code_hash      text COLLATE "C" CONSTRAINT oauth_codes_pkey PRIMARY KEY,
    client_id      text COLLATE "C" NOT NULL CONSTRAINT oauth_codes_client_id_fk REFERENCES oauth_clients (id) ON DELETE CASCADE,
    user_id        uuid NOT NULL CONSTRAINT oauth_codes_user_id_fk REFERENCES users (id) ON DELETE CASCADE,
    redirect_uri   text NOT NULL,
    scopes         jsonb NOT NULL DEFAULT '[]',
    code_challenge text NOT NULL,
    resource       text,
    created_at     timestamptz NOT NULL,
    expires_at     timestamptz NOT NULL,
    grant_id       uuid
);
CREATE INDEX oauth_codes_client_idx ON oauth_codes (client_id);
CREATE INDEX oauth_codes_user_idx ON oauth_codes (user_id);
CREATE INDEX oauth_codes_expires_idx ON oauth_codes (expires_at);

CREATE TABLE oauth_grants (
    id            uuid CONSTRAINT oauth_grants_pkey PRIMARY KEY,
    client_id     text COLLATE "C" NOT NULL CONSTRAINT oauth_grants_client_id_fk REFERENCES oauth_clients (id) ON DELETE CASCADE,
    user_id       uuid NOT NULL CONSTRAINT oauth_grants_user_id_fk REFERENCES users (id) ON DELETE CASCADE,
    scopes        jsonb NOT NULL DEFAULT '[]',
    refresh_hash  text COLLATE "C" NOT NULL,
    previous_hash text COLLATE "C",
    created_at    timestamptz NOT NULL,
    last_used_at  timestamptz NOT NULL,
    expires_at    timestamptz NOT NULL
);
CREATE UNIQUE INDEX oauth_grants_refresh_hash_uq ON oauth_grants (refresh_hash);
CREATE INDEX oauth_grants_previous_hash_idx ON oauth_grants (previous_hash) WHERE previous_hash IS NOT NULL;
CREATE INDEX oauth_grants_client_idx ON oauth_grants (client_id);
CREATE INDEX oauth_grants_user_idx ON oauth_grants (user_id, created_at);
CREATE INDEX oauth_grants_created_idx ON oauth_grants (created_at, id);
CREATE INDEX oauth_grants_expires_idx ON oauth_grants (expires_at);

ALTER TABLE api_tokens ADD COLUMN oauth_grant_id uuid
    CONSTRAINT api_tokens_oauth_grant_fk REFERENCES oauth_grants (id) ON DELETE CASCADE;
CREATE INDEX api_tokens_oauth_grant_idx ON api_tokens (oauth_grant_id);
