-- ferrite:phase expand
-- Indexed lookup of media files by variant hash (ruling R29). One row per entry of
-- `media.variants` whose value is an object with a string `hash`; kept in sync by the
-- media repository on every insert/update (same transaction) and removed with the media
-- row. `media.variants` stays the source of truth; this table is derived from it.

CREATE TABLE media_variants (
    hash    TEXT NOT NULL,
    item_id BLOB NOT NULL
            CONSTRAINT media_variants_item_id_fk REFERENCES media (item_id) ON DELETE CASCADE,
    name    TEXT NOT NULL,
    CONSTRAINT media_variants_pkey PRIMARY KEY (item_id, name)
);
CREATE INDEX media_variants_hash_idx ON media_variants (hash);

-- Backfill from the existing JSON (same rule as the repository's sync).
INSERT INTO media_variants (hash, item_id, name)
SELECT json_extract(v.value, '$.hash'), m.item_id, v.key
FROM media AS m, json_each(m.variants) AS v
WHERE json_type(m.variants) = 'object'
  AND v.type = 'object'
  AND json_type(v.value, '$.hash') = 'text';
