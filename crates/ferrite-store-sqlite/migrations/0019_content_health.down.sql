-- ferrite:lossy-down
-- Reverse of 0019_content_health.up.sql. Lossy (declared in the up header, where the
-- runner reads it): every suppression and each type's publish gate go; the health runs,
-- findings and link checks are derived and rebuilt by the next run after an up.
DROP TABLE link_checks;
DROP TABLE health_findings;
DROP TABLE health_runs;
DROP TABLE quality_suppressions;
ALTER TABLE content_types DROP COLUMN quality_gate;
