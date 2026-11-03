-- ferrite:lossy-down
-- Reverse of 0017_releases_workflows.up.sql. Lossy (declared in the up header, where the
-- runner reads it): every release, staged change, workflow, review and preview link is lost.
DROP TABLE preview_links;
DROP TABLE review_assignees;
DROP TABLE review_decisions;
DROP TABLE reviews;
DROP TABLE workflows;
DROP TABLE release_locks;
DROP TABLE release_items;
DROP TABLE releases;
