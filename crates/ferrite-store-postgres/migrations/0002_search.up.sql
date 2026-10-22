-- ferrite:phase expand
-- Full-text search (02-storage §4, R17). The store normalizes text in Rust and writes
-- `tsv` as a tsvector literal (title lexemes weighted A, body B), so no text search
-- parser, configuration or locale is involved. Rows cascade with the content item.

CREATE TABLE search_docs (
    item_id uuid CONSTRAINT search_docs_pkey PRIMARY KEY
            CONSTRAINT search_docs_item_id_fk REFERENCES content_items (id) ON DELETE CASCADE,
    tsv     tsvector NOT NULL
);
CREATE INDEX search_docs_tsv_idx ON search_docs USING GIN (tsv);
