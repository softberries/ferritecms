-- ferrite:lossy-down
-- Reverse of 0015_plugin_publishers.up.sql. Lossy (declared in the up header, where the
-- runner reads it): the owner bindings, the trusted keys and every revocation go, so every
-- plugin name becomes unsigned-owned again and a revoked key is trusted again after an
-- up. No plugin, its data or its secrets are removed; the registry cache is refetched.
DROP TABLE plugin_registry;
DROP TABLE plugin_owners;
DROP TABLE plugin_publishers;
