-- ferrite:phase expand
-- Per-node minute metrics (M4-2 P35). One row per node and minute (unix micros of a
-- whole UTC minute), written by every node from its registry's deltas; a second write of
-- the same minute adds to the row. No foreign key to `nodes`: a node's rows outlive its
-- registry row, for the 7-day retention (`node_metrics.prune`). The minute index serves
-- the series ranges and the prune.

CREATE TABLE node_metrics (
    node_id         BLOB NOT NULL,
    minute          INTEGER NOT NULL,
    requests        INTEGER NOT NULL,
    errors_5xx      INTEGER NOT NULL,
    page_views      INTEGER NOT NULL,
    cache_hits      INTEGER NOT NULL,
    cache_misses    INTEGER NOT NULL,
    plugin_calls    INTEGER NOT NULL,
    plugin_failures INTEGER NOT NULL,
    jobs_done       INTEGER NOT NULL,
    jobs_failed     INTEGER NOT NULL,
    p95_ms          INTEGER NOT NULL,
    CONSTRAINT node_metrics_pkey PRIMARY KEY (node_id, minute)
);
CREATE INDEX node_metrics_minute_idx ON node_metrics (minute);
