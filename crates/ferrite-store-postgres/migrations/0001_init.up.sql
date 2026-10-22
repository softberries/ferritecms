-- ferrite:phase expand
-- Core schema (02-storage §4). Postgres physical types (§5): uuid ids, timestamptz,
-- jsonb, boolean, bytea. Requires PostgreSQL >= 15 (UNIQUE NULLS NOT DISTINCT).
-- Key, slug, name, login and title columns are COLLATE "C" (byte order, like SQLite's
-- BINARY), so their indexes serve ORDER BY without per-query collation clauses (R8).

CREATE TABLE content_types (
    name         text COLLATE "C" CONSTRAINT content_types_pkey PRIMARY KEY,
    label        text NOT NULL,
    hierarchical boolean NOT NULL,
    has_archive  boolean NOT NULL,
    fields       jsonb NOT NULL,
    built_in     boolean NOT NULL DEFAULT false,
    created_at   timestamptz NOT NULL
);

CREATE TABLE users (
    id            uuid CONSTRAINT users_pkey PRIMARY KEY,
    login         text COLLATE "C" NOT NULL,
    email         text NOT NULL,
    -- ascii_lowercase(email), computed by the store: identical folding on every backend.
    email_key     text COLLATE "C" NOT NULL,
    display_name  text NOT NULL,
    password_hash text,
    role          text NOT NULL,
    status        text NOT NULL,
    must_reset    boolean NOT NULL DEFAULT false,
    totp_secret   text,
    created_at    timestamptz NOT NULL,
    last_login_at timestamptz
);
CREATE UNIQUE INDEX users_login_uq ON users (login);
-- Case-insensitive e-mail uniqueness.
CREATE UNIQUE INDEX users_email_uq ON users (email_key);

CREATE TABLE content_items (
    id           uuid CONSTRAINT content_items_pkey PRIMARY KEY,
    type         text COLLATE "C" NOT NULL CONSTRAINT content_items_type_fk REFERENCES content_types (name),
    status       text NOT NULL
                 CHECK (status IN ('draft', 'pending', 'scheduled', 'published', 'private', 'trash')),
    slug         text COLLATE "C" NOT NULL,
    parent_id    uuid CONSTRAINT content_items_parent_id_fk REFERENCES content_items (id),
    author_id    uuid NOT NULL CONSTRAINT content_items_author_id_fk REFERENCES users (id),
    title        text COLLATE "C" NOT NULL,
    excerpt      text NOT NULL,
    body         jsonb NOT NULL,
    body_html    text,
    fields       jsonb NOT NULL,
    menu_order   integer NOT NULL DEFAULT 0,
    published_at timestamptz,
    scheduled_at timestamptz,
    created_at   timestamptz NOT NULL,
    updated_at   timestamptz NOT NULL,
    version      bigint NOT NULL DEFAULT 1,
    -- NULL parent counts as one value, so two top-level items cannot share a slug.
    CONSTRAINT content_items_slug_uq UNIQUE NULLS NOT DISTINCT (type, parent_id, slug)
);
-- Listing indexes: one per ContentOrder, columns and directions exactly as its ORDER BY
-- (content repo `push_part`), so a page is an index range scan with no sort.
CREATE INDEX content_items_published_list_idx
    ON content_items (type, status, published_at DESC NULLS LAST, id DESC);
CREATE INDEX content_items_updated_list_idx ON content_items (type, updated_at DESC, id DESC);
CREATE INDEX content_items_menu_order_list_idx ON content_items (type, menu_order, id);
CREATE INDEX content_items_title_list_idx ON content_items (type, title, id);
CREATE INDEX content_items_parent_idx ON content_items (parent_id);
CREATE INDEX content_items_author_idx ON content_items (author_id);
CREATE INDEX content_items_status_scheduled_idx ON content_items (status, scheduled_at);

CREATE TABLE content_meta (
    item_id uuid NOT NULL CONSTRAINT content_meta_item_id_fk REFERENCES content_items (id) ON DELETE CASCADE,
    key     text COLLATE "C" NOT NULL,
    value   jsonb NOT NULL,
    CONSTRAINT content_meta_pkey PRIMARY KEY (item_id, key)
);

CREATE TABLE revisions (
    id         uuid CONSTRAINT revisions_pkey PRIMARY KEY,
    item_id    uuid NOT NULL CONSTRAINT revisions_item_id_fk REFERENCES content_items (id) ON DELETE CASCADE,
    author_id  uuid NOT NULL CONSTRAINT revisions_author_id_fk REFERENCES users (id),
    created_at timestamptz NOT NULL,
    title      text NOT NULL,
    excerpt    text NOT NULL,
    body       jsonb NOT NULL,
    fields     jsonb NOT NULL,
    meta       jsonb NOT NULL,
    note       text
);
CREATE INDEX revisions_item_created_idx ON revisions (item_id, created_at);

CREATE TABLE taxonomies (
    name          text COLLATE "C" CONSTRAINT taxonomies_pkey PRIMARY KEY,
    label         text NOT NULL,
    hierarchical  boolean NOT NULL,
    content_types jsonb NOT NULL,
    built_in      boolean NOT NULL DEFAULT false
);

CREATE TABLE terms (
    id          uuid CONSTRAINT terms_pkey PRIMARY KEY,
    taxonomy    text COLLATE "C" NOT NULL CONSTRAINT terms_taxonomy_fk REFERENCES taxonomies (name),
    slug        text COLLATE "C" NOT NULL,
    name        text COLLATE "C" NOT NULL,
    description text NOT NULL,
    parent_id   uuid CONSTRAINT terms_parent_id_fk REFERENCES terms (id)
);
CREATE UNIQUE INDEX terms_taxonomy_slug_uq ON terms (taxonomy, slug);
CREATE INDEX terms_parent_idx ON terms (parent_id);

CREATE TABLE term_links (
    item_id  uuid NOT NULL CONSTRAINT term_links_item_id_fk REFERENCES content_items (id) ON DELETE CASCADE,
    term_id  uuid NOT NULL CONSTRAINT term_links_term_id_fk REFERENCES terms (id) ON DELETE CASCADE,
    position integer NOT NULL,
    CONSTRAINT term_links_pkey PRIMARY KEY (item_id, term_id)
);
CREATE INDEX term_links_term_idx ON term_links (term_id);

CREATE TABLE menus (
    name  text COLLATE "C" CONSTRAINT menus_pkey PRIMARY KEY,
    label text NOT NULL
);

CREATE TABLE menu_items (
    id        uuid CONSTRAINT menu_items_pkey PRIMARY KEY,
    menu      text COLLATE "C" NOT NULL CONSTRAINT menu_items_menu_fk REFERENCES menus (name) ON DELETE CASCADE,
    parent_id uuid CONSTRAINT menu_items_parent_id_fk REFERENCES menu_items (id) ON DELETE CASCADE,
    position  integer NOT NULL,
    kind      text NOT NULL CHECK (kind IN ('item', 'term', 'url')),
    target_id uuid,
    url       text,
    label     text NOT NULL
);
CREATE INDEX menu_items_menu_idx ON menu_items (menu, parent_id, position);

CREATE TABLE artifacts (
    hash       text COLLATE "C" CONSTRAINT artifacts_pkey PRIMARY KEY,
    kind       text NOT NULL CHECK (kind IN ('media', 'variant', 'plugin', 'theme', 'export')),
    bytes      bigint NOT NULL,
    mime       text NOT NULL,
    created_at timestamptz NOT NULL
);

CREATE TABLE media (
    item_id       uuid CONSTRAINT media_pkey PRIMARY KEY CONSTRAINT media_item_id_fk REFERENCES content_items (id) ON DELETE CASCADE,
    artifact_hash text COLLATE "C" NOT NULL CONSTRAINT media_artifact_hash_fk REFERENCES artifacts (hash),
    mime          text NOT NULL,
    bytes         bigint NOT NULL,
    width         integer,
    height        integer,
    alt           text NOT NULL,
    original_name text NOT NULL,
    variants      jsonb NOT NULL
);
CREATE INDEX media_artifact_idx ON media (artifact_hash);

CREATE TABLE comments (
    id           uuid CONSTRAINT comments_pkey PRIMARY KEY,
    item_id      uuid NOT NULL CONSTRAINT comments_item_id_fk REFERENCES content_items (id) ON DELETE CASCADE,
    parent_id    uuid CONSTRAINT comments_parent_id_fk REFERENCES comments (id) ON DELETE CASCADE,
    user_id      uuid CONSTRAINT comments_user_id_fk REFERENCES users (id) ON DELETE SET NULL,
    author_name  text NOT NULL,
    author_email text NOT NULL,
    author_url   text NOT NULL,
    body         text NOT NULL,
    status       text NOT NULL,
    ip_hash      text,
    created_at   timestamptz NOT NULL
);
CREATE INDEX comments_item_created_idx ON comments (item_id, created_at);

CREATE TABLE redirects (
    id          uuid CONSTRAINT redirects_pkey PRIMARY KEY,
    source_path text COLLATE "C" NOT NULL,
    target      text NOT NULL,
    code        integer NOT NULL CHECK (code IN (301, 302, 307, 308, 410)),
    hits        bigint NOT NULL DEFAULT 0,
    origin      text NOT NULL CHECK (origin IN ('manual', 'import', 'slug-change')),
    created_at  timestamptz NOT NULL,
    last_hit_at timestamptz
);
CREATE UNIQUE INDEX redirects_source_path_uq ON redirects (source_path);
CREATE INDEX redirects_target_idx ON redirects (target);

CREATE TABLE user_meta (
    user_id uuid NOT NULL CONSTRAINT user_meta_user_id_fk REFERENCES users (id) ON DELETE CASCADE,
    key     text COLLATE "C" NOT NULL,
    value   jsonb NOT NULL,
    CONSTRAINT user_meta_pkey PRIMARY KEY (user_id, key)
);

CREATE TABLE options (
    key        text COLLATE "C" CONSTRAINT options_pkey PRIMARY KEY,
    value      jsonb NOT NULL,
    autoload   boolean NOT NULL DEFAULT false,
    updated_at timestamptz NOT NULL
);

CREATE TABLE plugins (
    name            text COLLATE "C" CONSTRAINT plugins_pkey PRIMARY KEY,
    version         text NOT NULL,
    artifact_hash   text COLLATE "C" NOT NULL CONSTRAINT plugins_artifact_hash_fk REFERENCES artifacts (hash),
    manifest        jsonb NOT NULL,
    granted         jsonb NOT NULL,
    enabled         boolean NOT NULL DEFAULT false,
    disabled_reason text,
    installed_at    timestamptz NOT NULL,
    updated_at      timestamptz NOT NULL
);

CREATE TABLE plugin_kv (
    plugin     text COLLATE "C" NOT NULL,
    key        text COLLATE "C" NOT NULL,
    value      bytea NOT NULL,
    updated_at timestamptz NOT NULL,
    CONSTRAINT plugin_kv_pkey PRIMARY KEY (plugin, key)
);

CREATE TABLE plugin_failures (
    plugin       text COLLATE "C" NOT NULL,
    hook         text COLLATE "C" NOT NULL,
    bucket_start timestamptz NOT NULL,
    count        bigint NOT NULL,
    CONSTRAINT plugin_failures_pkey PRIMARY KEY (plugin, hook, bucket_start)
);

CREATE TABLE themes (
    name          text COLLATE "C" CONSTRAINT themes_pkey PRIMARY KEY,
    version       text NOT NULL,
    artifact_hash text COLLATE "C" NOT NULL CONSTRAINT themes_artifact_hash_fk REFERENCES artifacts (hash),
    manifest      jsonb NOT NULL,
    active        boolean NOT NULL DEFAULT false,
    installed_at  timestamptz NOT NULL
);
-- At most one active theme.
CREATE UNIQUE INDEX themes_one_active_uq ON themes (active) WHERE active;

CREATE TABLE events (
    seq         bigint GENERATED ALWAYS AS IDENTITY CONSTRAINT events_pkey PRIMARY KEY,
    kind        text COLLATE "C" NOT NULL,
    keys        jsonb NOT NULL,
    origin_node text,
    created_at  timestamptz NOT NULL
);
CREATE INDEX events_created_idx ON events (created_at);

-- site_id and created_by_version are written by the migration runner, not here.
CREATE TABLE schema_meta (
    key   text COLLATE "C" CONSTRAINT schema_meta_pkey PRIMARY KEY,
    value text NOT NULL
);

INSERT INTO content_types (name, label, hierarchical, has_archive, fields, built_in, created_at) VALUES
    ('post', 'Posts', false, true, '{}', true, now()),
    ('page', 'Pages', true, false, '{}', true, now()),
    ('media', 'Media', false, false, '{}', true, now());

INSERT INTO taxonomies (name, label, hierarchical, content_types, built_in) VALUES
    ('category', 'Categories', true, '["post"]', true),
    ('tag', 'Tags', false, '["post"]', true);
