-- Reverse of 0013_oauth.up.sql. Not declared lossy, like 0012: everything in these tables
-- is an authorisation a user repeats in one click (a client connects again), and no
-- content is removed. The OAuth access tokens are deleted first, so they cannot turn
-- into personal tokens.
DELETE FROM api_tokens WHERE oauth_grant_id IS NOT NULL;
DROP INDEX api_tokens_oauth_grant_idx;

-- SQLite cannot drop a column that carries a foreign key: the same rebuild as 0005's
-- table, without the column. Nothing references `api_tokens`.
CREATE TABLE api_tokens_0013 AS
SELECT id, user_id, name, token_hash, scopes, created_at, last_used_at, expires_at
FROM api_tokens;
DROP TABLE api_tokens;
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
INSERT INTO api_tokens (id, user_id, name, token_hash, scopes, created_at, last_used_at, expires_at)
SELECT id, user_id, name, token_hash, scopes, created_at, last_used_at, expires_at
FROM api_tokens_0013;
DROP TABLE api_tokens_0013;
CREATE UNIQUE INDEX api_tokens_hash_uq ON api_tokens (token_hash);
CREATE INDEX api_tokens_user_idx ON api_tokens (user_id);

DROP TABLE oauth_grants;
DROP TABLE oauth_codes;
DROP TABLE oauth_requests;
DROP TABLE oauth_clients;
