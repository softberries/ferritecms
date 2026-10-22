-- Reverse of 0003_media_variants.up.sql. Not lossy: the rows are derived from
-- `media.variants`, which is untouched, and the up migration rebuilds them.
DROP TABLE media_variants;
