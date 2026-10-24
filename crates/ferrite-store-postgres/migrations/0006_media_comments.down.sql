-- ferrite:lossy-down
-- Reverse of 0006_media_comments.up.sql. Lossy (declared in the up header, where the
-- runner reads it): idempotency keys are lost, and every sanitised SVG loses its marker
-- (it is served as an attachment again).
DROP INDEX jobs_created_idx;
DROP INDEX jobs_status_created_idx;
DROP INDEX comments_user_idx;
DROP INDEX comments_author_email_idx;
DROP INDEX comments_created_idx;
DROP INDEX comments_item_status_idx;
DROP INDEX comments_status_created_idx;
ALTER TABLE media DROP COLUMN sanitized;
DROP TABLE idempotency_keys;
