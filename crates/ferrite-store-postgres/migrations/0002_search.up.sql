-- ferrite:phase expand
-- Full-text search (02-storage §4): one row per indexed item; the title is weighted A and
-- the body B. v1 uses the 'simple' configuration (locale-specific configs: M1c).

CREATE TABLE search_docs (
    item_id   uuid PRIMARY KEY REFERENCES content_items (id) ON DELETE CASCADE,
    title     text NOT NULL,
    body_text text NOT NULL,
    tsv       tsvector GENERATED ALWAYS AS (
        setweight(to_tsvector('simple', title), 'A') ||
        setweight(to_tsvector('simple', body_text), 'B')
    ) STORED
);
CREATE INDEX search_docs_tsv_idx ON search_docs USING GIN (tsv);
