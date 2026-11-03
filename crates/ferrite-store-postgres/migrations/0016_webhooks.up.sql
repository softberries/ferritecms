-- ferrite:phase expand
-- Outgoing webhooks (design 18 §7). `webhook_endpoints`: the URLs an administrator
-- registered, their subscriptions (patterns as jsonb), the sealed signing secret (and the
-- previous one during a rotation), the install id they were created or last enabled under
-- (a copied database pauses them) and their health. `webhook_events`: one row per event,
-- whatever the number of endpoints. `webhook_deliveries`: one event for one endpoint;
-- `redelivery_of` has no foreign key (the original may be pruned). `webhook_attempts`: one
-- row per try of a delivery. Text keys are COLLATE "C" (R8).

CREATE TABLE webhook_endpoints (
    id                         uuid CONSTRAINT webhook_endpoints_pkey PRIMARY KEY,
    name                       text NOT NULL,
    url                        text NOT NULL,
    events                     jsonb NOT NULL,
    content_types              jsonb,
    enabled                    boolean NOT NULL,
    disabled_reason            text CHECK (disabled_reason IN ('manual', 'failing', 'gone')),
    disabled_at                timestamptz,
    install_id                 text COLLATE "C" NOT NULL,
    secret_sealed              text NOT NULL,
    previous_secret_sealed     text,
    previous_secret_expires_at timestamptz,
    payload_version            integer NOT NULL DEFAULT 1,
    failing_since              timestamptz,
    consecutive_failures       integer NOT NULL DEFAULT 0,
    last_success_at            timestamptz,
    last_failure_at            timestamptz,
    created_by                 uuid CONSTRAINT webhook_endpoints_created_by_fk REFERENCES users (id) ON DELETE SET NULL,
    created_at                 timestamptz NOT NULL,
    updated_at                 timestamptz NOT NULL
);
CREATE INDEX webhook_endpoints_created_by_idx ON webhook_endpoints (created_by);

CREATE TABLE webhook_events (
    id         uuid CONSTRAINT webhook_events_pkey PRIMARY KEY,
    type       text COLLATE "C" NOT NULL,
    data       jsonb NOT NULL,
    created_at timestamptz NOT NULL
);
CREATE INDEX webhook_events_created_idx ON webhook_events (created_at);

CREATE TABLE webhook_deliveries (
    id               uuid CONSTRAINT webhook_deliveries_pkey PRIMARY KEY,
    endpoint_id      uuid NOT NULL CONSTRAINT webhook_deliveries_endpoint_id_fk REFERENCES webhook_endpoints (id) ON DELETE CASCADE,
    event_id         uuid NOT NULL CONSTRAINT webhook_deliveries_event_id_fk REFERENCES webhook_events (id) ON DELETE CASCADE,
    status           text COLLATE "C" NOT NULL CHECK (status IN ('pending', 'delivered', 'failed', 'skipped')),
    attempts         integer NOT NULL DEFAULT 0,
    next_attempt_at  timestamptz,
    last_status_code integer,
    last_error       text,
    redelivery_of    uuid,
    created_at       timestamptz NOT NULL,
    finished_at      timestamptz
);
CREATE INDEX webhook_deliveries_endpoint_idx ON webhook_deliveries (endpoint_id, created_at DESC, id DESC);
CREATE INDEX webhook_deliveries_event_idx ON webhook_deliveries (event_id);

CREATE TABLE webhook_attempts (
    delivery_id      uuid NOT NULL CONSTRAINT webhook_attempts_delivery_id_fk REFERENCES webhook_deliveries (id) ON DELETE CASCADE,
    attempt          integer NOT NULL,
    started_at       timestamptz NOT NULL,
    duration_ms      integer NOT NULL,
    status_code      integer,
    error            text,
    response_excerpt text,
    CONSTRAINT webhook_attempts_pkey PRIMARY KEY (delivery_id, attempt)
);
