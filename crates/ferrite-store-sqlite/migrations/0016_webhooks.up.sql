-- ferrite:phase expand
-- Outgoing webhooks (design 18 §7). `webhook_endpoints`: the URLs an administrator
-- registered, their subscriptions (patterns as JSON), the sealed signing secret (and the
-- previous one during a rotation), the install id they were created or last enabled under
-- (a copied database pauses them) and their health. `webhook_events`: one row per event,
-- whatever the number of endpoints. `webhook_deliveries`: one event for one endpoint;
-- `redelivery_of` has no foreign key (the original may be pruned). `webhook_attempts`: one
-- row per try of a delivery.

CREATE TABLE webhook_endpoints (
    id                         BLOB CONSTRAINT webhook_endpoints_pkey PRIMARY KEY NOT NULL,
    name                       TEXT NOT NULL,
    url                        TEXT NOT NULL,
    events                     TEXT NOT NULL CHECK (json_valid(events)),
    content_types              TEXT CHECK (content_types IS NULL OR json_valid(content_types)),
    enabled                    INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    disabled_reason            TEXT CHECK (disabled_reason IN ('manual', 'failing', 'gone')),
    disabled_at                INTEGER,
    install_id                 TEXT NOT NULL,
    secret_sealed              TEXT NOT NULL,
    previous_secret_sealed     TEXT,
    previous_secret_expires_at INTEGER,
    payload_version            INTEGER NOT NULL DEFAULT 1,
    failing_since              INTEGER,
    consecutive_failures       INTEGER NOT NULL DEFAULT 0,
    last_success_at            INTEGER,
    last_failure_at            INTEGER,
    created_by                 BLOB CONSTRAINT webhook_endpoints_created_by_fk REFERENCES users (id) ON DELETE SET NULL,
    created_at                 INTEGER NOT NULL,
    updated_at                 INTEGER NOT NULL
);
CREATE INDEX webhook_endpoints_created_by_idx ON webhook_endpoints (created_by);

CREATE TABLE webhook_events (
    id         BLOB CONSTRAINT webhook_events_pkey PRIMARY KEY NOT NULL,
    type       TEXT NOT NULL,
    data       TEXT NOT NULL CHECK (json_valid(data)),
    created_at INTEGER NOT NULL
);
CREATE INDEX webhook_events_created_idx ON webhook_events (created_at);

CREATE TABLE webhook_deliveries (
    id               BLOB CONSTRAINT webhook_deliveries_pkey PRIMARY KEY NOT NULL,
    endpoint_id      BLOB NOT NULL CONSTRAINT webhook_deliveries_endpoint_id_fk REFERENCES webhook_endpoints (id) ON DELETE CASCADE,
    event_id         BLOB NOT NULL CONSTRAINT webhook_deliveries_event_id_fk REFERENCES webhook_events (id) ON DELETE CASCADE,
    status           TEXT NOT NULL CHECK (status IN ('pending', 'delivered', 'failed', 'skipped')),
    attempts         INTEGER NOT NULL DEFAULT 0,
    next_attempt_at  INTEGER,
    last_status_code INTEGER,
    last_error       TEXT,
    redelivery_of    BLOB,
    created_at       INTEGER NOT NULL,
    finished_at      INTEGER
);
CREATE INDEX webhook_deliveries_endpoint_idx ON webhook_deliveries (endpoint_id, created_at DESC, id DESC);
CREATE INDEX webhook_deliveries_event_idx ON webhook_deliveries (event_id);

CREATE TABLE webhook_attempts (
    delivery_id      BLOB NOT NULL CONSTRAINT webhook_attempts_delivery_id_fk REFERENCES webhook_deliveries (id) ON DELETE CASCADE,
    attempt          INTEGER NOT NULL,
    started_at       INTEGER NOT NULL,
    duration_ms      INTEGER NOT NULL,
    status_code      INTEGER,
    error            TEXT,
    response_excerpt TEXT,
    CONSTRAINT webhook_attempts_pkey PRIMARY KEY (delivery_id, attempt)
);
