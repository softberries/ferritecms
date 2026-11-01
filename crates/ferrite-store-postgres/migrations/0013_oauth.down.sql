-- Reverse of 0013_oauth.up.sql. Not declared lossy, like 0012: everything in these tables
-- is an authorisation a user repeats in one click (a client connects again), and no
-- content is removed. The OAuth access tokens are deleted first, so they cannot turn
-- into personal tokens.
DELETE FROM api_tokens WHERE oauth_grant_id IS NOT NULL;
DROP INDEX api_tokens_oauth_grant_idx;
ALTER TABLE api_tokens DROP COLUMN oauth_grant_id;
DROP TABLE oauth_grants;
DROP TABLE oauth_codes;
DROP TABLE oauth_requests;
DROP TABLE oauth_clients;
