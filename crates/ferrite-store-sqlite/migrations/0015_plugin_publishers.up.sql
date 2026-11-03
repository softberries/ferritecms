-- ferrite:phase expand
-- ferrite:lossy-down   -- the down drops the owner bindings, trusted keys and revocations
-- Plugin trust (M6-2, design 14 §6). `plugin_publishers`: the minisign keys this site
-- trusts on its own (approved at install, added by the operator) and every revocation
-- (any source; a registry key has a row only once revoked). `plugin_owners`: which
-- publisher identity a plugin name's data belongs to (`owner` NULL = unsigned) and how its
-- current version arrived; no foreign key to `plugins`, it outlives the row while the data
-- is retained. `plugin_registry`: the last verified index of each registry URL (a cache).
-- No rows are written: a name without an owner row is unsigned-owned (D13).

CREATE TABLE plugin_publishers (
    key_id         TEXT CONSTRAINT plugin_publishers_pkey PRIMARY KEY NOT NULL,
    public_key     TEXT NOT NULL,
    label          TEXT NOT NULL,
    source         TEXT NOT NULL CHECK (source IN ('approved', 'operator', 'registry')),
    trusted_at     INTEGER NOT NULL,
    revoked_at     INTEGER,
    revoked_reason TEXT,
    compromised    INTEGER NOT NULL DEFAULT 0 CHECK (compromised IN (0, 1))
);
CREATE UNIQUE INDEX plugin_publishers_public_key_uq ON plugin_publishers (public_key);

CREATE TABLE plugin_owners (
    plugin       TEXT CONSTRAINT plugin_owners_pkey PRIMARY KEY NOT NULL,
    owner        TEXT,
    signer_key   TEXT,
    signer_public_key TEXT,
    signature    TEXT,
    publisher    TEXT NOT NULL,
    source       TEXT NOT NULL CHECK (source IN ('upload', 'folder', 'registry')),
    registry_url TEXT,
    bound_at     INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL
);
CREATE INDEX plugin_owners_signer_idx ON plugin_owners (signer_key);

CREATE TABLE plugin_registry (
    url           TEXT CONSTRAINT plugin_registry_pkey PRIMARY KEY NOT NULL,
    serial        INTEGER NOT NULL,
    index_json    TEXT NOT NULL,
    signature     TEXT NOT NULL,
    generated_at  INTEGER NOT NULL,
    fetched_at    INTEGER NOT NULL,
    checked_at    INTEGER NOT NULL,
    last_error    TEXT,
    last_error_at INTEGER
);
