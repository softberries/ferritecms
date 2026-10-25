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
-- Text keys are COLLATE "C" (R8).

CREATE TABLE autosaves (
    item_id      uuid NOT NULL CONSTRAINT autosaves_item_id_fk REFERENCES content_items (id) ON DELETE CASCADE,
    user_id      uuid NOT NULL CONSTRAINT autosaves_user_id_fk REFERENCES users (id) ON DELETE CASCADE,
    base_version bigint NOT NULL,
    title        text COLLATE "C" NOT NULL,
    excerpt      text,
    slug         text COLLATE "C",
    parent_id    uuid,
    menu_order   integer,
    terms        jsonb,
    body         jsonb NOT NULL,
    fields       jsonb NOT NULL,
    saved_at     timestamptz NOT NULL,
    CONSTRAINT autosaves_pkey PRIMARY KEY (item_id, user_id)
);
CREATE INDEX autosaves_user_idx ON autosaves (user_id);
