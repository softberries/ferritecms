-- ferrite:phase expand
-- Core schema (02-storage §4). SQLite physical types (§5): ids are 16-byte UUIDv7 BLOBs,
-- timestamps INTEGER unix microseconds UTC, json validated TEXT, bools INTEGER 0/1.

CREATE TABLE content_types (
    name         TEXT PRIMARY KEY NOT NULL,
    label        TEXT NOT NULL,
    hierarchical INTEGER NOT NULL CHECK (hierarchical IN (0, 1)),
    has_archive  INTEGER NOT NULL CHECK (has_archive IN (0, 1)),
    fields       TEXT NOT NULL CHECK (json_valid(fields)),
    built_in     INTEGER NOT NULL DEFAULT 0 CHECK (built_in IN (0, 1)),
    created_at   INTEGER NOT NULL
);

CREATE TABLE users (
    id            BLOB PRIMARY KEY NOT NULL,
    login         TEXT NOT NULL,
    email         TEXT NOT NULL COLLATE NOCASE,
    display_name  TEXT NOT NULL,
    password_hash TEXT,
    role          TEXT NOT NULL,
    status        TEXT NOT NULL,
    must_reset    INTEGER NOT NULL DEFAULT 0 CHECK (must_reset IN (0, 1)),
    totp_secret   TEXT,
    created_at    INTEGER NOT NULL,
    last_login_at INTEGER
);
CREATE UNIQUE INDEX users_login_uq ON users (login);
-- Uses the column's NOCASE collation: e-mail uniqueness is case-insensitive.
CREATE UNIQUE INDEX users_email_uq ON users (email);

CREATE TABLE content_items (
    id           BLOB PRIMARY KEY NOT NULL,
    type         TEXT NOT NULL REFERENCES content_types (name),
    status       TEXT NOT NULL
                 CHECK (status IN ('draft', 'pending', 'scheduled', 'published', 'private', 'trash')),
    slug         TEXT NOT NULL,
    parent_id    BLOB REFERENCES content_items (id),
    author_id    BLOB NOT NULL REFERENCES users (id),
    title        TEXT NOT NULL,
    excerpt      TEXT NOT NULL,
    body         TEXT NOT NULL CHECK (json_valid(body)),
    body_html    TEXT,
    fields       TEXT NOT NULL CHECK (json_valid(fields)),
    menu_order   INTEGER NOT NULL DEFAULT 0,
    published_at INTEGER,
    scheduled_at INTEGER,
    created_at   INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL,
    version      INTEGER NOT NULL DEFAULT 1
);
-- NULL parent counts as one value, so two top-level items cannot share a slug.
CREATE UNIQUE INDEX content_items_slug_uq ON content_items (type, coalesce(parent_id, x''), slug);
CREATE INDEX content_items_type_status_published_idx ON content_items (type, status, published_at);
CREATE INDEX content_items_parent_idx ON content_items (parent_id);
CREATE INDEX content_items_author_idx ON content_items (author_id);
CREATE INDEX content_items_status_scheduled_idx ON content_items (status, scheduled_at);

CREATE TABLE content_meta (
    item_id BLOB NOT NULL REFERENCES content_items (id) ON DELETE CASCADE,
    key     TEXT NOT NULL,
    value   TEXT NOT NULL CHECK (json_valid(value)),
    PRIMARY KEY (item_id, key)
);

CREATE TABLE revisions (
    id         BLOB PRIMARY KEY NOT NULL,
    item_id    BLOB NOT NULL REFERENCES content_items (id) ON DELETE CASCADE,
    author_id  BLOB NOT NULL REFERENCES users (id),
    created_at INTEGER NOT NULL,
    title      TEXT NOT NULL,
    excerpt    TEXT NOT NULL,
    body       TEXT NOT NULL CHECK (json_valid(body)),
    fields     TEXT NOT NULL CHECK (json_valid(fields)),
    meta       TEXT NOT NULL CHECK (json_valid(meta)),
    note       TEXT
);
CREATE INDEX revisions_item_created_idx ON revisions (item_id, created_at);

CREATE TABLE taxonomies (
    name          TEXT PRIMARY KEY NOT NULL,
    label         TEXT NOT NULL,
    hierarchical  INTEGER NOT NULL CHECK (hierarchical IN (0, 1)),
    content_types TEXT NOT NULL CHECK (json_valid(content_types)),
    built_in      INTEGER NOT NULL DEFAULT 0 CHECK (built_in IN (0, 1))
);

CREATE TABLE terms (
    id          BLOB PRIMARY KEY NOT NULL,
    taxonomy    TEXT NOT NULL REFERENCES taxonomies (name),
    slug        TEXT NOT NULL,
    name        TEXT NOT NULL,
    description TEXT NOT NULL,
    parent_id   BLOB REFERENCES terms (id) ON DELETE SET NULL
);
CREATE UNIQUE INDEX terms_taxonomy_slug_uq ON terms (taxonomy, slug);
CREATE INDEX terms_parent_idx ON terms (parent_id);

CREATE TABLE term_links (
    item_id  BLOB NOT NULL REFERENCES content_items (id) ON DELETE CASCADE,
    term_id  BLOB NOT NULL REFERENCES terms (id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    PRIMARY KEY (item_id, term_id)
);
CREATE INDEX term_links_term_idx ON term_links (term_id);

CREATE TABLE menus (
    name  TEXT PRIMARY KEY NOT NULL,
    label TEXT NOT NULL
);

CREATE TABLE menu_items (
    id        BLOB PRIMARY KEY NOT NULL,
    menu      TEXT NOT NULL REFERENCES menus (name) ON DELETE CASCADE,
    parent_id BLOB REFERENCES menu_items (id) ON DELETE CASCADE,
    position  INTEGER NOT NULL,
    kind      TEXT NOT NULL CHECK (kind IN ('item', 'term', 'url')),
    target_id BLOB,
    url       TEXT,
    label     TEXT NOT NULL
);
CREATE INDEX menu_items_menu_idx ON menu_items (menu, parent_id, position);

CREATE TABLE artifacts (
    hash       TEXT PRIMARY KEY NOT NULL,
    kind       TEXT NOT NULL CHECK (kind IN ('media', 'variant', 'plugin', 'theme', 'export')),
    bytes      INTEGER NOT NULL,
    mime       TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE TABLE media (
    item_id       BLOB PRIMARY KEY NOT NULL REFERENCES content_items (id) ON DELETE CASCADE,
    artifact_hash TEXT NOT NULL REFERENCES artifacts (hash),
    mime          TEXT NOT NULL,
    bytes         INTEGER NOT NULL,
    width         INTEGER,
    height        INTEGER,
    alt           TEXT NOT NULL,
    original_name TEXT NOT NULL,
    variants      TEXT NOT NULL CHECK (json_valid(variants))
);
CREATE INDEX media_artifact_idx ON media (artifact_hash);

CREATE TABLE comments (
    id           BLOB PRIMARY KEY NOT NULL,
    item_id      BLOB NOT NULL REFERENCES content_items (id) ON DELETE CASCADE,
    parent_id    BLOB REFERENCES comments (id) ON DELETE CASCADE,
    user_id      BLOB REFERENCES users (id) ON DELETE SET NULL,
    author_name  TEXT NOT NULL,
    author_email TEXT NOT NULL,
    author_url   TEXT NOT NULL,
    body         TEXT NOT NULL,
    status       TEXT NOT NULL,
    ip_hash      TEXT,
    created_at   INTEGER NOT NULL
);
CREATE INDEX comments_item_created_idx ON comments (item_id, created_at);

CREATE TABLE redirects (
    id          BLOB PRIMARY KEY NOT NULL,
    source_path TEXT NOT NULL,
    target      TEXT NOT NULL,
    code        INTEGER NOT NULL CHECK (code IN (301, 302, 307, 308, 410)),
    hits        INTEGER NOT NULL DEFAULT 0,
    origin      TEXT NOT NULL CHECK (origin IN ('manual', 'import', 'slug-change')),
    created_at  INTEGER NOT NULL,
    last_hit_at INTEGER
);
CREATE UNIQUE INDEX redirects_source_path_uq ON redirects (source_path);

CREATE TABLE user_meta (
    user_id BLOB NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    key     TEXT NOT NULL,
    value   TEXT NOT NULL CHECK (json_valid(value)),
    PRIMARY KEY (user_id, key)
);

CREATE TABLE options (
    key        TEXT PRIMARY KEY NOT NULL,
    value      TEXT NOT NULL CHECK (json_valid(value)),
    autoload   INTEGER NOT NULL DEFAULT 0 CHECK (autoload IN (0, 1)),
    updated_at INTEGER NOT NULL
);

CREATE TABLE plugins (
    name            TEXT PRIMARY KEY NOT NULL,
    version         TEXT NOT NULL,
    artifact_hash   TEXT NOT NULL REFERENCES artifacts (hash),
    manifest        TEXT NOT NULL CHECK (json_valid(manifest)),
    granted         TEXT NOT NULL CHECK (json_valid(granted)),
    enabled         INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0, 1)),
    disabled_reason TEXT,
    installed_at    INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL
);

CREATE TABLE plugin_kv (
    plugin     TEXT NOT NULL,
    key        TEXT NOT NULL,
    value      BLOB NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (plugin, key)
);

CREATE TABLE plugin_failures (
    plugin       TEXT NOT NULL,
    hook         TEXT NOT NULL,
    bucket_start INTEGER NOT NULL,
    count        INTEGER NOT NULL,
    PRIMARY KEY (plugin, hook, bucket_start)
);

CREATE TABLE themes (
    name          TEXT PRIMARY KEY NOT NULL,
    version       TEXT NOT NULL,
    artifact_hash TEXT NOT NULL REFERENCES artifacts (hash),
    manifest      TEXT NOT NULL CHECK (json_valid(manifest)),
    active        INTEGER NOT NULL DEFAULT 0 CHECK (active IN (0, 1)),
    installed_at  INTEGER NOT NULL
);
-- At most one active theme.
CREATE UNIQUE INDEX themes_one_active_uq ON themes (active) WHERE active = 1;

CREATE TABLE events (
    seq         INTEGER PRIMARY KEY AUTOINCREMENT,
    kind        TEXT NOT NULL,
    keys        TEXT NOT NULL CHECK (json_valid(keys)),
    origin_node TEXT,
    created_at  INTEGER NOT NULL
);
CREATE INDEX events_created_idx ON events (created_at);

-- site_id and created_by_version are written by the migration runner, not here.
CREATE TABLE schema_meta (
    key   TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
);

INSERT INTO content_types (name, label, hierarchical, has_archive, fields, built_in, created_at) VALUES
    ('post', 'Posts', 0, 1, '{}', 1, CAST(unixepoch('subsec') * 1000000 AS INTEGER)),
    ('page', 'Pages', 1, 0, '{}', 1, CAST(unixepoch('subsec') * 1000000 AS INTEGER)),
    ('media', 'Media', 0, 0, '{}', 1, CAST(unixepoch('subsec') * 1000000 AS INTEGER));

INSERT INTO taxonomies (name, label, hierarchical, content_types, built_in) VALUES
    ('category', 'Categories', 1, '["post"]', 1),
    ('tag', 'Tags', 0, '["post"]', 1);
