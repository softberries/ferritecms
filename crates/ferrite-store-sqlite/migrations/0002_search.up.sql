-- ferrite:phase expand
-- Full-text search (02-storage §4, R17). The store normalizes text in Rust and stores
-- each field as space-separated tokens in `search_docs`; `search_fts` is an external-
-- content FTS5 index over it with the `ascii` tokenizer, so FTS5 only splits on those
-- spaces. Rows cascade with the content item and the triggers keep the index in sync.

CREATE TABLE search_docs (
    id        INTEGER PRIMARY KEY,
    item_id   BLOB NOT NULL UNIQUE
              CONSTRAINT search_docs_item_id_fk REFERENCES content_items (id) ON DELETE CASCADE,
    title     TEXT NOT NULL,
    body_text TEXT NOT NULL
);

CREATE VIRTUAL TABLE search_fts USING fts5 (
    title,
    body_text,
    content = 'search_docs',
    content_rowid = 'id',
    tokenize = 'ascii'
);

CREATE TRIGGER search_docs_ai AFTER INSERT ON search_docs BEGIN
    INSERT INTO search_fts (rowid, title, body_text)
    VALUES (new.id, new.title, new.body_text);
END;

CREATE TRIGGER search_docs_ad AFTER DELETE ON search_docs BEGIN
    INSERT INTO search_fts (search_fts, rowid, title, body_text)
    VALUES ('delete', old.id, old.title, old.body_text);
END;

CREATE TRIGGER search_docs_au AFTER UPDATE ON search_docs BEGIN
    INSERT INTO search_fts (search_fts, rowid, title, body_text)
    VALUES ('delete', old.id, old.title, old.body_text);
    INSERT INTO search_fts (rowid, title, body_text)
    VALUES (new.id, new.title, new.body_text);
END;
