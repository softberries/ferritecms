-- ferrite:phase expand
-- ferrite:lossy-down   -- the down drops every release, workflow, review and preview link
-- Editing (design 15 §8). `releases`: named sets of changes applied in one transaction;
-- `release_items`: one entry per (release, item) with an optional staged change (a jsonb
-- snapshot, D1) and a transition; `release_locks`: one row per item of a scheduled
-- release (the primary key keeps an item in at most one, D5); `workflows`: a content
-- type's review steps; `reviews`: one round of approval of an item revision or a
-- release entry (at most one open per target, the partial unique index), with its
-- decisions and assignees; `preview_links`: hashed, expiring preview tokens (D26).
-- Text keys are COLLATE "C" (R8). Status columns have no CHECK: rows decode only with a
-- known value (P2).

CREATE TABLE releases (
    id              uuid CONSTRAINT releases_pkey PRIMARY KEY,
    title           text NOT NULL,
    description     text NOT NULL DEFAULT '',
    status          text COLLATE "C" NOT NULL,
    scheduled_at    timestamptz,
    scheduled_by    uuid CONSTRAINT releases_scheduled_by_fk REFERENCES users (id) ON DELETE SET NULL,
    override_reason text,
    failure         jsonb,
    created_by      uuid CONSTRAINT releases_created_by_fk REFERENCES users (id) ON DELETE SET NULL,
    created_at      timestamptz NOT NULL,
    updated_at      timestamptz NOT NULL,
    applied_at      timestamptz,
    applied_by      uuid CONSTRAINT releases_applied_by_fk REFERENCES users (id) ON DELETE SET NULL,
    version         bigint NOT NULL DEFAULT 1
);
CREATE INDEX releases_status_scheduled_idx ON releases (status, scheduled_at);
CREATE INDEX releases_created_idx ON releases (created_at DESC, id DESC);
CREATE INDEX releases_created_by_idx ON releases (created_by);
CREATE INDEX releases_scheduled_by_idx ON releases (scheduled_by);
CREATE INDEX releases_applied_by_idx ON releases (applied_by);

CREATE TABLE release_items (
    release_id    uuid NOT NULL CONSTRAINT release_items_release_id_fk REFERENCES releases (id) ON DELETE CASCADE,
    item_id       uuid NOT NULL CONSTRAINT release_items_item_id_fk REFERENCES content_items (id) ON DELETE CASCADE,
    position      integer NOT NULL,
    transition    text COLLATE "C" NOT NULL,
    changes       jsonb,
    base_version  bigint NOT NULL,
    entry_version bigint NOT NULL DEFAULT 1,
    added_by      uuid CONSTRAINT release_items_added_by_fk REFERENCES users (id) ON DELETE SET NULL,
    updated_at    timestamptz NOT NULL,
    CONSTRAINT release_items_pkey PRIMARY KEY (release_id, item_id)
);
CREATE INDEX release_items_item_idx ON release_items (item_id);
CREATE INDEX release_items_added_by_idx ON release_items (added_by);

CREATE TABLE release_locks (
    item_id    uuid CONSTRAINT release_locks_pkey PRIMARY KEY
               CONSTRAINT release_locks_item_id_fk REFERENCES content_items (id) ON DELETE CASCADE,
    release_id uuid NOT NULL CONSTRAINT release_locks_release_id_fk REFERENCES releases (id) ON DELETE CASCADE
);
CREATE INDEX release_locks_release_idx ON release_locks (release_id);

CREATE TABLE workflows (
    type       text COLLATE "C" CONSTRAINT workflows_pkey PRIMARY KEY
               CONSTRAINT workflows_type_fk REFERENCES content_types (name) ON DELETE CASCADE,
    steps      jsonb NOT NULL,
    updated_by uuid CONSTRAINT workflows_updated_by_fk REFERENCES users (id) ON DELETE SET NULL,
    updated_at timestamptz NOT NULL,
    version    bigint NOT NULL DEFAULT 1
);
CREATE INDEX workflows_updated_by_idx ON workflows (updated_by);

CREATE TABLE reviews (
    id            uuid CONSTRAINT reviews_pkey PRIMARY KEY,
    target        text COLLATE "C" NOT NULL,
    item_id       uuid NOT NULL CONSTRAINT reviews_item_id_fk REFERENCES content_items (id) ON DELETE CASCADE,
    release_id    uuid,
    revision_id   uuid,
    entry_version bigint,
    status        text COLLATE "C" NOT NULL,
    step          integer NOT NULL DEFAULT 0,
    steps         jsonb NOT NULL,
    note          text,
    submitted_by  uuid CONSTRAINT reviews_submitted_by_fk REFERENCES users (id) ON DELETE SET NULL,
    submitted_at  timestamptz NOT NULL,
    decided_at    timestamptz,
    version       bigint NOT NULL DEFAULT 1,
    CONSTRAINT reviews_entry_fk FOREIGN KEY (release_id, item_id)
        REFERENCES release_items (release_id, item_id) ON DELETE CASCADE
);
CREATE UNIQUE INDEX reviews_open_target_uq ON reviews (target)
    WHERE status IN ('in_review', 'approved');
CREATE INDEX reviews_item_idx ON reviews (item_id);
CREATE INDEX reviews_status_idx ON reviews (status, submitted_at, id);
CREATE INDEX reviews_release_item_idx ON reviews (release_id, item_id);
CREATE INDEX reviews_submitted_by_idx ON reviews (submitted_by);

CREATE TABLE review_decisions (
    id        uuid CONSTRAINT review_decisions_pkey PRIMARY KEY,
    review_id uuid NOT NULL CONSTRAINT review_decisions_review_id_fk REFERENCES reviews (id) ON DELETE CASCADE,
    step      integer NOT NULL,
    user_id   uuid CONSTRAINT review_decisions_user_id_fk REFERENCES users (id) ON DELETE SET NULL,
    decision  text COLLATE "C" NOT NULL,
    comment   text,
    at        timestamptz NOT NULL
);
CREATE UNIQUE INDEX review_decisions_step_user_uq ON review_decisions (review_id, step, user_id);
CREATE INDEX review_decisions_user_idx ON review_decisions (user_id);

CREATE TABLE review_assignees (
    review_id uuid NOT NULL CONSTRAINT review_assignees_review_id_fk REFERENCES reviews (id) ON DELETE CASCADE,
    step      integer NOT NULL,
    user_id   uuid NOT NULL CONSTRAINT review_assignees_user_id_fk REFERENCES users (id) ON DELETE CASCADE,
    CONSTRAINT review_assignees_pkey PRIMARY KEY (review_id, step, user_id)
);
CREATE INDEX review_assignees_user_idx ON review_assignees (user_id);

CREATE TABLE preview_links (
    id           uuid CONSTRAINT preview_links_pkey PRIMARY KEY,
    token_hash   text COLLATE "C" NOT NULL,
    target_kind  text COLLATE "C" NOT NULL,
    target_id    uuid NOT NULL,
    created_by   uuid NOT NULL CONSTRAINT preview_links_created_by_fk REFERENCES users (id) ON DELETE CASCADE,
    created_at   timestamptz NOT NULL,
    expires_at   timestamptz NOT NULL,
    last_used_at timestamptz
);
CREATE UNIQUE INDEX preview_links_token_hash_uq ON preview_links (token_hash);
CREATE INDEX preview_links_target_idx ON preview_links (target_kind, target_id);
CREATE INDEX preview_links_expires_idx ON preview_links (expires_at);
CREATE INDEX preview_links_created_by_idx ON preview_links (created_by);
