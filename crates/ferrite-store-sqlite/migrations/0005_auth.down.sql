-- ferrite:lossy-down
-- Reverse of 0005_auth.up.sql. Lossy (declared in the up header, where the runner reads
-- it): sessions, API tokens, rate-limit counters, the audit log and the TOTP replay
-- guard are lost.
ALTER TABLE users DROP COLUMN totp_last_step;
DROP INDEX audit_log_actor_idx;
DROP TABLE audit_log;
DROP TABLE rate_limits;
DROP TABLE api_tokens;
DROP TABLE sessions;
