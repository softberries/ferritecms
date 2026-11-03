-- ferrite:phase expand
-- ferrite:lossy-down   -- the down drops every suppression and resets each type's publish gate
-- Content quality (M6-4, design 16 §7). `content_types.quality_gate`: the per-type publish
-- gate. `quality_suppressions`: findings a person accepted, with a reason (`key` '' covers
-- every finding of the rule on the item). `health_runs` and `health_findings`: the daily
-- content health run and the findings of the newest one. `link_checks`: the external link
-- cache shared by every node (`url_hash` is the SHA-256 of the URL). Runs, findings and
-- link checks are derived: the next run rebuilds them. Text keys are COLLATE "C" (R8).

ALTER TABLE content_types ADD COLUMN quality_gate text COLLATE "C" NOT NULL DEFAULT 'off'
    CHECK (quality_gate IN ('off', 'errors'));

CREATE TABLE quality_suppressions (
    item_id    uuid NOT NULL CONSTRAINT quality_suppressions_item_id_fk REFERENCES content_items (id) ON DELETE CASCADE,
    rule       text COLLATE "C" NOT NULL,
    key        text COLLATE "C" NOT NULL,
    reason     text NOT NULL,
    user_id    uuid CONSTRAINT quality_suppressions_user_id_fk REFERENCES users (id) ON DELETE SET NULL,
    created_at timestamptz NOT NULL,
    CONSTRAINT quality_suppressions_pkey PRIMARY KEY (item_id, rule, key)
);
CREATE INDEX quality_suppressions_user_idx ON quality_suppressions (user_id);

CREATE TABLE health_runs (
    id            uuid CONSTRAINT health_runs_pkey PRIMARY KEY,
    status        text COLLATE "C" NOT NULL CHECK (status IN ('running', 'done', 'failed', 'abandoned')),
    trigger       text COLLATE "C" NOT NULL CHECK (trigger IN ('schedule', 'manual')),
    requested_by  uuid CONSTRAINT health_runs_requested_by_fk REFERENCES users (id) ON DELETE SET NULL,
    node          text NOT NULL,
    started_at    timestamptz NOT NULL,
    finished_at   timestamptz,
    items_checked bigint NOT NULL DEFAULT 0,
    paths_checked bigint NOT NULL DEFAULT 0,
    urls_checked  bigint NOT NULL DEFAULT 0,
    totals        jsonb NOT NULL,
    truncated     boolean NOT NULL DEFAULT false,
    error         text
);
CREATE INDEX health_runs_status_idx ON health_runs (status, started_at);
CREATE INDEX health_runs_requested_by_idx ON health_runs (requested_by);

CREATE TABLE health_findings (
    run_id      uuid NOT NULL CONSTRAINT health_findings_run_id_fk REFERENCES health_runs (id) ON DELETE CASCADE,
    seq         bigint NOT NULL,
    rule        text COLLATE "C" NOT NULL,
    severity    text COLLATE "C" NOT NULL,
    category    text COLLATE "C" NOT NULL,
    item_id     uuid CONSTRAINT health_findings_item_id_fk REFERENCES content_items (id) ON DELETE CASCADE,
    redirect_id uuid CONSTRAINT health_findings_redirect_id_fk REFERENCES redirects (id) ON DELETE CASCADE,
    item_type   text COLLATE "C",
    locale      text COLLATE "C",
    key         text COLLATE "C" NOT NULL,
    context     text,
    detail      jsonb NOT NULL,
    CONSTRAINT health_findings_pkey PRIMARY KEY (run_id, seq)
);
CREATE INDEX health_findings_filter_idx ON health_findings (run_id, category, severity, seq);
CREATE INDEX health_findings_item_idx ON health_findings (item_id);
CREATE INDEX health_findings_redirect_idx ON health_findings (redirect_id);

CREATE TABLE link_checks (
    url_hash        bytea CONSTRAINT link_checks_pkey PRIMARY KEY,
    url             text NOT NULL,
    host            text COLLATE "C" NOT NULL,
    status          text COLLATE "C" NOT NULL CHECK (status IN ('ok', 'failing', 'broken', 'skipped')),
    http_status     integer,
    detail          text,
    failures        integer NOT NULL DEFAULT 0,
    first_failed_at timestamptz,
    checked_at      timestamptz NOT NULL,
    last_seen_at    timestamptz NOT NULL
);
CREATE INDEX link_checks_seen_idx ON link_checks (last_seen_at);
