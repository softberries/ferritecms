-- ferrite:phase expand
-- Full-text search (02-storage §4): an FTS5 index of each item's title and body text.
-- FTS5 tables have no foreign keys, so the store removes rows when content is deleted.
-- `item_id` is the item's 16-byte id BLOB; bm25 weights are (item_id, title, body_text).

CREATE VIRTUAL TABLE search_fts USING fts5 (
    item_id UNINDEXED,
    title,
    body_text
);
