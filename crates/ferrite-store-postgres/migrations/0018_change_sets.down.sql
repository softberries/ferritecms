-- Reverse of 0018_change_sets.up.sql. Not declared lossy, like 0016: the change sets are
-- a review log of assistant writes (pruned after `mcp.change_sets_days`); no content is
-- removed.
DROP TABLE change_set_changes;
DROP TABLE change_sets;
