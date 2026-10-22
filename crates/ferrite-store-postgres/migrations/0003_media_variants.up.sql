-- ferrite:phase expand
-- Indexed lookup of media files by variant hash (ruling R29). One row per entry of
-- `media.variants` whose value is an object with a string `hash`; kept in sync by the
-- media repository on every insert/update (same transaction) and removed with the media
-- row. `media.variants` stays the source of truth; this table is derived from it.

CREATE TABLE media_variants (
    hash    text COLLATE "C" NOT NULL,
    item_id uuid NOT NULL
            CONSTRAINT media_variants_item_id_fk REFERENCES media (item_id) ON DELETE CASCADE,
    name    text COLLATE "C" NOT NULL,
    CONSTRAINT media_variants_pkey PRIMARY KEY (item_id, name)
);
CREATE INDEX media_variants_hash_idx ON media_variants (hash);

-- Backfill from the existing JSON (same rule as the repository's sync).
INSERT INTO media_variants (hash, item_id, name)
SELECT v.value ->> 'hash', m.item_id, v.key
FROM media AS m
CROSS JOIN LATERAL jsonb_each(
    CASE WHEN jsonb_typeof(m.variants) = 'object' THEN m.variants ELSE '{}'::jsonb END
) AS v
WHERE jsonb_typeof(v.value) = 'object'
  AND jsonb_typeof(v.value -> 'hash') = 'string';
