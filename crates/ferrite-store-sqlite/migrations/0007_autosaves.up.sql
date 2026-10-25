-- ferrite:phase expand
-- ferrite:lossy-down   -- the down drops every unsaved autosave
-- The per-user autosave slot of a content item (M2b P15; F10, F11). At most one row per
-- (item, user): the admin's autosave overwrites it, the user's next real save of the
-- item deletes it, and deleting the item or the user deletes it with them (no
-- retention job). `excerpt` NULL = derive from the body; `parent_id` is the working
-- copy's parent, NULL = top level (ruling R4); `slug`, `menu_order` and `terms` NULL =
-- unchanged. `parent_id` has no foreign key and `terms` (a JSON array of term ids)
-- none either: both are checked on the real save. `autosaves_user_idx` serves the
-- users foreign key on delete.
-- `title` and `slug` use the default BINARY collation (parity with Postgres "C").

CREATE TABLE autosaves (
    item_id      BLOB NOT NULL CONSTRAINT autosaves_item_id_fk REFERENCES content_items (id) ON DELETE CASCADE,
    user_id      BLOB NOT NULL CONSTRAINT autosaves_user_id_fk REFERENCES users (id) ON DELETE CASCADE,
    base_version INTEGER NOT NULL,
    title        TEXT NOT NULL,
    excerpt      TEXT,
    slug         TEXT,
    parent_id    BLOB,
    menu_order   INTEGER,
    terms        TEXT CHECK (terms IS NULL OR json_valid(terms)),
    body         TEXT NOT NULL CHECK (json_valid(body)),
    fields       TEXT NOT NULL CHECK (json_valid(fields)),
    saved_at     INTEGER NOT NULL,
    CONSTRAINT autosaves_pkey PRIMARY KEY (item_id, user_id)
);
CREATE INDEX autosaves_user_idx ON autosaves (user_id);
