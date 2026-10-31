+++
description = "Run FerriteCMS in production: configuration, backups, upgrades, HTTPS, monitoring and clusters."
+++

# Operations

Running a site after the first day.

- [Configuration](01-configuration.md): every `ferrite.toml` key with its default, and
  the `FERRITE_*` environment overrides.
- [Backups and moves](02-backups.md): `backup` and `restore`, scheduled backups, the
  portable export, copying a database, media clean-up.
- [Upgrades and rollback](03-upgrades.md): `self-update`, migrations and going back.
- [HTTPS](04-https.md): behind a reverse proxy, or with built-in Let's Encrypt
  certificates.
- [Monitoring](05-monitoring.md): health endpoints, Prometheus metrics, the traffic
  series and `ferrite doctor`.
- [Cluster](06-cluster.md): several nodes on Postgres and shared media.
