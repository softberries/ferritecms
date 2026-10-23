-- ferrite:lossy-down
-- Reverse of 0004_cluster.up.sql. Lossy (declared in the up header, where the runner
-- reads it): queued and running jobs and the node registry are lost.
DROP TABLE nodes;
DROP TABLE jobs;
