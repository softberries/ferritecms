-- ferrite:phase expand
-- Per-node minute metrics (M4-2 P35). One row per node and minute (a whole UTC minute),
-- written by every node from its registry's deltas; a second write of the same minute
-- adds to the row. No foreign key to `nodes`: a node's rows outlive its registry row, for
-- the 7-day retention (`node_metrics.prune`). The minute index serves the series ranges
-- and the prune. `page_le_<ms>` / `page_over_10000` count the minute's page responses per
-- latency bucket (not cumulative), so a series sums them and takes the p95 of the combined
-- histogram (M4-2 R7).

CREATE TABLE node_metrics (
    node_id         uuid NOT NULL,
    minute          timestamptz NOT NULL,
    requests        bigint NOT NULL,
    errors_5xx      bigint NOT NULL,
    page_views      bigint NOT NULL,
    cache_hits      bigint NOT NULL,
    cache_misses    bigint NOT NULL,
    plugin_calls    bigint NOT NULL,
    plugin_failures bigint NOT NULL,
    jobs_done       bigint NOT NULL,
    jobs_failed     bigint NOT NULL,
    page_le_5       bigint NOT NULL,
    page_le_10      bigint NOT NULL,
    page_le_25      bigint NOT NULL,
    page_le_50      bigint NOT NULL,
    page_le_100     bigint NOT NULL,
    page_le_250     bigint NOT NULL,
    page_le_500     bigint NOT NULL,
    page_le_1000    bigint NOT NULL,
    page_le_2500    bigint NOT NULL,
    page_le_5000    bigint NOT NULL,
    page_le_10000   bigint NOT NULL,
    page_over_10000 bigint NOT NULL,
    CONSTRAINT node_metrics_pkey PRIMARY KEY (node_id, minute)
);
CREATE INDEX node_metrics_minute_idx ON node_metrics (minute);
