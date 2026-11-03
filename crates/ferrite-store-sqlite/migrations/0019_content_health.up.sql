-- ferrite:phase expand
-- ferrite:lossy-down   -- the down drops every suppression and resets each type's publish gate
-- Content quality (M6-4, design 16 §7). `content_types.quality_gate`: the per-type publish
-- gate. `quality_suppressions`: findings a person accepted, with a reason (`key` '' covers
-- every finding of the rule on the item). `health_runs` and `health_findings`: the daily
-- content health run and the findings of the newest one. `link_checks`: the external link
-- cache shared by every node (`url_hash` is the SHA-256 of the URL). Runs, findings and
-- link checks are derived: the next run rebuilds them.

ALTER TABLE content_types ADD COLUMN quality_gate TEXT NOT NULL DEFAULT 'off'
    CHECK (quality_gate IN ('off', 'errors'));

CREATE TABLE quality_suppressions (
    item_id    BLOB NOT NULL CONSTRAINT quality_suppressions_item_id_fk REFERENCES content_items (id) ON DELETE CASCADE,
    rule       TEXT NOT NULL,
    key        TEXT NOT NULL,
    reason     TEXT NOT NULL,
    user_id    BLOB CONSTRAINT quality_suppressions_user_id_fk REFERENCES users (id) ON DELETE SET NULL,
    created_at INTEGER NOT NULL,
    CONSTRAINT quality_suppressions_pkey PRIMARY KEY (item_id, rule, key)
);
CREATE INDEX quality_suppressions_user_idx ON quality_suppressions (user_id);

CREATE TABLE health_runs (
    id            BLOB CONSTRAINT health_runs_pkey PRIMARY KEY NOT NULL,
    status        TEXT NOT NULL CHECK (status IN ('running', 'done', 'failed', 'abandoned')),
    trigger       TEXT NOT NULL CHECK (trigger IN ('schedule', 'manual')),
    requested_by  BLOB CONSTRAINT health_runs_requested_by_fk REFERENCES users (id) ON DELETE SET NULL,
    node          TEXT NOT NULL,
    started_at    INTEGER NOT NULL,
    finished_at   INTEGER,
    items_checked INTEGER NOT NULL DEFAULT 0,
    paths_checked INTEGER NOT NULL DEFAULT 0,
    urls_checked  INTEGER NOT NULL DEFAULT 0,
    totals        TEXT NOT NULL CHECK (json_valid(totals)),
    truncated     INTEGER NOT NULL DEFAULT 0 CHECK (truncated IN (0, 1)),
    error         TEXT
);
CREATE INDEX health_runs_status_idx ON health_runs (status, started_at);
CREATE INDEX health_runs_requested_by_idx ON health_runs (requested_by);

CREATE TABLE health_findings (
    run_id      BLOB NOT NULL CONSTRAINT health_findings_run_id_fk REFERENCES health_runs (id) ON DELETE CASCADE,
    seq         INTEGER NOT NULL,
    rule        TEXT NOT NULL,
    severity    TEXT NOT NULL,
    category    TEXT NOT NULL,
    item_id     BLOB CONSTRAINT health_findings_item_id_fk REFERENCES content_items (id) ON DELETE CASCADE,
    redirect_id BLOB CONSTRAINT health_findings_redirect_id_fk REFERENCES redirects (id) ON DELETE CASCADE,
    item_type   TEXT,
    locale      TEXT,
    key         TEXT NOT NULL,
    context     TEXT,
    detail      TEXT NOT NULL CHECK (json_valid(detail)),
    CONSTRAINT health_findings_pkey PRIMARY KEY (run_id, seq)
);
CREATE INDEX health_findings_filter_idx ON health_findings (run_id, category, severity, seq);
CREATE INDEX health_findings_item_idx ON health_findings (item_id);
CREATE INDEX health_findings_redirect_idx ON health_findings (redirect_id);

CREATE TABLE link_checks (
    url_hash        BLOB CONSTRAINT link_checks_pkey PRIMARY KEY NOT NULL,
    url             TEXT NOT NULL,
    host            TEXT NOT NULL,
    status          TEXT NOT NULL CHECK (status IN ('ok', 'failing', 'broken', 'skipped')),
    http_status     INTEGER,
    detail          TEXT,
    failures        INTEGER NOT NULL DEFAULT 0,
    first_failed_at INTEGER,
    checked_at      INTEGER NOT NULL,
    last_seen_at    INTEGER NOT NULL
);
CREATE INDEX link_checks_seen_idx ON link_checks (last_seen_at);
