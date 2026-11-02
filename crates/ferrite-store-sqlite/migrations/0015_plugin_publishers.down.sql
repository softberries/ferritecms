-- Reverse of 0015_plugin_publishers.up.sql. Not declared lossy: the bindings and the
-- trusted keys go, and every plugin name becomes unsigned-owned again, as before M6-2; no
-- plugin, its data or its secrets are removed, and the registry cache is refetched.
DROP TABLE plugin_registry;
DROP INDEX plugin_owners_signer_idx;
DROP TABLE plugin_owners;
DROP INDEX plugin_publishers_public_key_uq;
DROP TABLE plugin_publishers;
