-- ferrite:phase expand
-- Plugin trust (M6-2, design 14 §6). `plugin_publishers`: the minisign keys this site
-- trusts on its own (approved at install, added by the operator) and every revocation
-- (any source; a registry key has a row only once revoked). `plugin_owners`: which
-- publisher identity a plugin name's data belongs to (`owner` NULL = unsigned) and how its
-- current version arrived; no foreign key to `plugins`, it outlives the row while the data
-- is retained. `plugin_registry`: the last verified index of each registry URL (a cache).
-- No rows are written: a name without an owner row is unsigned-owned (D13). Text keys are
-- COLLATE "C" (R8).

CREATE TABLE plugin_publishers (
    key_id         text COLLATE "C" CONSTRAINT plugin_publishers_pkey PRIMARY KEY,
    public_key     text COLLATE "C" NOT NULL,
    label          text NOT NULL,
    source         text NOT NULL CHECK (source IN ('approved', 'operator', 'registry')),
    trusted_at     timestamptz NOT NULL,
    revoked_at     timestamptz,
    revoked_reason text,
    compromised    boolean NOT NULL DEFAULT false
);
CREATE UNIQUE INDEX plugin_publishers_public_key_uq ON plugin_publishers (public_key);

CREATE TABLE plugin_owners (
    plugin       text COLLATE "C" CONSTRAINT plugin_owners_pkey PRIMARY KEY,
    owner        text COLLATE "C",
    signer_key   text COLLATE "C",
    signature    text,
    publisher    text NOT NULL,
    source       text NOT NULL CHECK (source IN ('upload', 'folder', 'registry')),
    registry_url text,
    bound_at     timestamptz NOT NULL,
    updated_at   timestamptz NOT NULL
);
CREATE INDEX plugin_owners_signer_idx ON plugin_owners (signer_key);

CREATE TABLE plugin_registry (
    url           text COLLATE "C" CONSTRAINT plugin_registry_pkey PRIMARY KEY,
    serial        bigint NOT NULL,
    index_json    text NOT NULL,
    signature     text NOT NULL,
    generated_at  timestamptz NOT NULL,
    fetched_at    timestamptz NOT NULL,
    checked_at    timestamptz NOT NULL,
    last_error    text,
    last_error_at timestamptz
);
