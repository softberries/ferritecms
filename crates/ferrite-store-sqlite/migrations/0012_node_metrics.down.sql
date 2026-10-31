-- Reverse of 0012_node_metrics.up.sql. Not declared lossy: the rows are transient
-- metrics (kept 7 days, not exported), and the nodes write new ones.
DROP INDEX node_metrics_minute_idx;
DROP TABLE node_metrics;
