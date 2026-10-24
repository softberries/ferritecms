//! Artifacts, media, installed plugins, plugin key-value storage and themes.

use std::collections::HashMap;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ferrite_domain::{
    Artifact, ArtifactHash, ContentId, InstalledPlugin, InstalledTheme, Media, Timestamp,
};
use ferrite_store::{
    ArtifactRepo, MAX_ID_FILTER, MediaRepo, PluginKvRepo, PluginRepo, StoreError, ThemeRepo,
    check_len, check_limit, check_prune_limit, validate_json, validate_text,
};

use sqlx::{Postgres, QueryBuilder, Row, types::Json};

use super::PgRepos;
use crate::{
    codec::{self, ARTIFACT_COLUMNS, MEDIA_COLUMNS, PLUGIN_COLUMNS, THEME_COLUMNS},
    exec::{atomic, map_db_err, run},
};

/// First key of the two-argument advisory lock taken per plugin by `set_with_quota`
/// (second key: `hashtext(plugin)`). The class keeps these locks apart from any other
/// advisory lock in the database.
const KV_LOCK_CLASS: i32 = 0x4652_4b56; // "FRKV"

#[async_trait]
impl ArtifactRepo for PgRepos {
    async fn insert_if_absent(&self, artifact: &Artifact) -> Result<bool, StoreError> {
        validate_text("artifact mime", &artifact.mime)?;
        let done = run!(&self.exec, |e| {
            sqlx::query(
            "INSERT INTO artifacts (hash, kind, bytes, mime, created_at) VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT DO NOTHING"
        )
        .bind(artifact.hash.as_str())
        .bind(artifact.kind.as_str())
        .bind(artifact.bytes)
        .bind(&artifact.mime)
        .bind(DateTime::<Utc>::from(artifact.created_at))
        .execute(e)
        })?;
        Ok(done.rows_affected() == 1)
    }

    async fn get(&self, hash: &ArtifactHash) -> Result<Option<Artifact>, StoreError> {
        let row = run!(&self.exec, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {ARTIFACT_COLUMNS} FROM artifacts WHERE hash = $1"
        )))
        .bind(hash.as_str())
        .fetch_optional(e))?;
        row.as_ref().map(codec::artifact).transpose()
    }

    async fn delete(&self, hash: &ArtifactHash) -> Result<(), StoreError> {
        run!(&self.exec, |e| sqlx::query(
            "DELETE FROM artifacts WHERE hash = $1"
        )
        .bind(hash.as_str())
        .execute(e))
        .map(|_| ())
    }
}

#[async_trait]
impl MediaRepo for PgRepos {
    async fn insert(&self, media: &Media) -> Result<(), StoreError> {
        validate_text("media mime", &media.mime)?;
        validate_text("media alt", &media.alt)?;
        validate_text("media original_name", &media.original_name)?;
        validate_json(&media.variants)?;
        let variants = Json(&media.variants);
        // The row and its `media_variants` rows commit together.
        atomic!(&self.exec, |conn| async {
            sqlx::query(
                "INSERT INTO media (item_id, artifact_hash, mime, bytes, width, height, alt, \
                 original_name, variants, sanitized) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
            )
            .bind(media.item_id.as_uuid())
            .bind(media.artifact_hash.as_str())
            .bind(&media.mime)
            .bind(media.bytes)
            .bind(media.width)
            .bind(media.height)
            .bind(&media.alt)
            .bind(&media.original_name)
            .bind(variants)
            .bind(media.sanitized)
            .execute(&mut *conn)
            .await
            .map_err(crate::exec::map_db_err)?;
            sync_variants(&mut *conn, media.item_id).await
        })
    }

    async fn get(&self, id: ContentId) -> Result<Option<Media>, StoreError> {
        let row = run!(&self.exec, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {MEDIA_COLUMNS} FROM media WHERE item_id = $1"
        )))
        .bind(id.as_uuid())
        .fetch_optional(e))?;
        row.as_ref().map(codec::media).transpose()
    }

    async fn update(&self, media: &Media) -> Result<(), StoreError> {
        validate_text("media mime", &media.mime)?;
        validate_text("media alt", &media.alt)?;
        validate_json(&media.variants)?;
        let variants = Json(&media.variants);
        atomic!(&self.exec, |conn| async {
            let done = sqlx::query(
                "UPDATE media SET mime = $1, bytes = $2, width = $3, height = $4, \
                 alt = $5, variants = $6, sanitized = $7 WHERE item_id = $8",
            )
            .bind(&media.mime)
            .bind(media.bytes)
            .bind(media.width)
            .bind(media.height)
            .bind(&media.alt)
            .bind(variants)
            .bind(media.sanitized)
            .bind(media.item_id.as_uuid())
            .execute(&mut *conn)
            .await
            .map_err(crate::exec::map_db_err)?;
            if done.rows_affected() == 0 {
                return Err(StoreError::NotFound);
            }
            sqlx::query("DELETE FROM media_variants WHERE item_id = $1")
                .bind(media.item_id.as_uuid())
                .execute(&mut *conn)
                .await
                .map_err(crate::exec::map_db_err)?;
            sync_variants(&mut *conn, media.item_id).await
        })
    }

    async fn get_many(&self, ids: &[ContentId]) -> Result<Vec<Media>, StoreError> {
        check_len("ids", ids.len(), MAX_ID_FILTER)?;
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut qb = QueryBuilder::<Postgres>::new(format!(
            "SELECT {MEDIA_COLUMNS} FROM media WHERE item_id IN ("
        ));
        let mut list = qb.separated(", ");
        for id in ids {
            list.push_bind(id.as_uuid());
        }
        list.push_unseparated(")");
        let rows = run!(&self.exec, |e| qb.build().fetch_all(e))?;
        let mut found = rows
            .iter()
            .map(codec::media)
            .map(|r| r.map(|m| (m.item_id, m)))
            .collect::<Result<HashMap<_, _>, _>>()?;
        Ok(ids.iter().filter_map(|id| found.remove(id)).collect())
    }

    async fn find_by_artifact(
        &self,
        hash: &ArtifactHash,
        limit: u32,
    ) -> Result<Vec<Media>, StoreError> {
        let limit = check_limit(limit)?;
        let rows = run!(&self.exec, |e| sqlx::query(FIND_MEDIA_BY_ARTIFACT_SQL)
            .bind(hash.as_str())
            .bind(i64::from(limit))
            .fetch_all(e))?;
        rows.iter().map(codec::media).collect()
    }
}

/// `MediaRepo::find_by_artifact` (ruling R29): nothing unless `artifacts` has the hash
/// as a media or variant file (primary key), then the indexed original match
/// (`media_artifact_idx`) and the indexed variant match (`media_variants_hash_idx`).
/// Bind 1: the hash; bind 2: the limit. Public so the plan test can `EXPLAIN` it.
pub const FIND_MEDIA_BY_ARTIFACT_SQL: &str = "SELECT item_id, artifact_hash, mime, bytes, width, height, alt, original_name, \
variants, sanitized FROM media \
WHERE EXISTS (SELECT 1 FROM artifacts WHERE hash = $1 AND kind IN ('media', 'variant')) \
AND item_id IN (SELECT item_id FROM media WHERE artifact_hash = $1 \
UNION SELECT item_id FROM media_variants WHERE hash = $1) \
ORDER BY item_id LIMIT $2";

/// Rebuilds the `media_variants` rows of `item` from its `variants` JSON (the rule of
/// migration 0003's backfill): one row per object value with a string `hash`.
const SYNC_VARIANTS_SQL: &str = "INSERT INTO media_variants (hash, item_id, name) \
SELECT v.value ->> 'hash', m.item_id, v.key FROM media AS m \
CROSS JOIN LATERAL jsonb_each(CASE WHEN jsonb_typeof(m.variants) = 'object' \
THEN m.variants ELSE '{}'::jsonb END) AS v \
WHERE m.item_id = $1 AND jsonb_typeof(v.value) = 'object' \
AND jsonb_typeof(v.value -> 'hash') = 'string'";

async fn sync_variants(conn: &mut sqlx::PgConnection, item: ContentId) -> Result<(), StoreError> {
    sqlx::query(SYNC_VARIANTS_SQL)
        .bind(item.as_uuid())
        .execute(conn)
        .await
        .map_err(crate::exec::map_db_err)?;
    Ok(())
}

#[async_trait]
impl PluginRepo for PgRepos {
    async fn upsert(&self, plugin: &InstalledPlugin) -> Result<(), StoreError> {
        validate_text("plugin name", &plugin.name)?;
        validate_text("plugin version", &plugin.version)?;
        if let Some(reason) = &plugin.disabled_reason {
            validate_text("plugin disabled_reason", reason)?;
        }
        validate_json(&plugin.manifest)?;
        validate_json(&plugin.granted)?;
        // A replaced plugin keeps its original install time.
        run!(&self.exec, |e| sqlx::query(
            "INSERT INTO plugins (name, version, artifact_hash, manifest, granted, enabled, \
             disabled_reason, installed_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) \
             ON CONFLICT (name) DO UPDATE SET version = excluded.version, \
             artifact_hash = excluded.artifact_hash, manifest = excluded.manifest, \
             granted = excluded.granted, enabled = excluded.enabled, \
             disabled_reason = excluded.disabled_reason, updated_at = excluded.updated_at"
        )
        .bind(&plugin.name)
        .bind(&plugin.version)
        .bind(plugin.artifact_hash.as_str())
        .bind(Json(&plugin.manifest))
        .bind(Json(&plugin.granted))
        .bind(plugin.enabled)
        .bind(plugin.disabled_reason.as_deref())
        .bind(DateTime::<Utc>::from(plugin.installed_at))
        .bind(DateTime::<Utc>::from(plugin.updated_at))
        .execute(e))
        .map(|_| ())
    }

    async fn get(&self, id: &str) -> Result<Option<InstalledPlugin>, StoreError> {
        validate_text("plugin name", id)?;
        let row = run!(&self.exec, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {PLUGIN_COLUMNS} FROM plugins WHERE name = $1"
        )))
        .bind(id)
        .fetch_optional(e))?;
        row.as_ref().map(codec::plugin).transpose()
    }

    async fn is_enabled(&self, id: &str) -> Result<Option<bool>, StoreError> {
        validate_text("plugin name", id)?;
        let row = run!(&self.exec, |e| sqlx::query(
            "SELECT enabled FROM plugins WHERE name = $1"
        )
        .bind(id)
        .fetch_optional(e))?;
        row.map(|r| r.try_get::<bool, _>("enabled").map_err(map_db_err))
            .transpose()
    }

    async fn get_for_update(&self, id: &str) -> Result<Option<InstalledPlugin>, StoreError> {
        validate_text("plugin name", id)?;
        let row = run!(&self.exec, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {PLUGIN_COLUMNS} FROM plugins WHERE name = $1 FOR UPDATE"
        )))
        .bind(id)
        .fetch_optional(e))?;
        row.as_ref().map(codec::plugin).transpose()
    }

    async fn list(&self) -> Result<Vec<InstalledPlugin>, StoreError> {
        let rows = run!(&self.exec, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {PLUGIN_COLUMNS} FROM plugins ORDER BY name COLLATE \"C\""
        )))
        .fetch_all(e))?;
        rows.iter().map(codec::plugin).collect()
    }

    async fn action_subscribers(&self, hook: &str) -> Result<Vec<String>, StoreError> {
        validate_text("hook", hook)?;
        let rows = run!(&self.exec, |e| sqlx::query(
            "SELECT name FROM plugins WHERE enabled \
             AND jsonb_typeof(manifest -> 'hooks') = 'object' \
             AND jsonb_exists(manifest -> 'hooks', $1) \
             ORDER BY name COLLATE \"C\""
        )
        .bind(hook)
        .fetch_all(e))?;
        rows.iter()
            .map(|r| r.try_get::<String, _>("name").map_err(map_db_err))
            .collect()
    }

    async fn set_enabled(
        &self,
        id: &str,
        enabled: bool,
        reason: Option<&str>,
    ) -> Result<(), StoreError> {
        validate_text("plugin name", id)?;
        if let Some(reason) = reason {
            validate_text("plugin disabled_reason", reason)?;
        }
        let done = run!(&self.exec, |e| {
            sqlx::query(
            "UPDATE plugins SET enabled = $1, disabled_reason = $2, updated_at = $3 WHERE name = $4"
        )
        .bind(enabled)
        .bind(reason)
        .bind(DateTime::<Utc>::from(Timestamp::now()))
        .bind(id)
        .execute(e)
        })?;
        if done.rows_affected() == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    async fn delete(&self, id: &str) -> Result<(), StoreError> {
        validate_text("plugin name", id)?;
        run!(&self.exec, |e| sqlx::query(
            "DELETE FROM plugins WHERE name = $1"
        )
        .bind(id)
        .execute(e))
        .map(|_| ())
    }

    async fn auto_disable(
        &self,
        id: &str,
        reason: &str,
        observed_updated_at: Timestamp,
    ) -> Result<bool, StoreError> {
        validate_text("plugin name", id)?;
        validate_text("plugin disabled_reason", reason)?;
        let done = run!(&self.exec, |e| sqlx::query(
            "UPDATE plugins SET enabled = false, disabled_reason = $2, updated_at = now() \
             WHERE name = $1 AND enabled AND updated_at = $3"
        )
        .bind(id)
        .bind(reason)
        .bind(DateTime::<Utc>::from(observed_updated_at))
        .execute(e))?;
        Ok(done.rows_affected() == 1)
    }
}

#[async_trait]
impl PluginKvRepo for PgRepos {
    async fn get(&self, plugin: &str, key: &str) -> Result<Option<Vec<u8>>, StoreError> {
        validate_text("plugin kv plugin", plugin)?;
        validate_text("plugin kv key", key)?;
        run!(&self.exec, |e| sqlx::query_scalar(
            "SELECT value FROM plugin_kv WHERE plugin = $1 AND key = $2"
        )
        .bind(plugin)
        .bind(key)
        .fetch_optional(e))
    }

    async fn set(&self, plugin: &str, key: &str, value: &[u8]) -> Result<(), StoreError> {
        validate_text("plugin kv plugin", plugin)?;
        validate_text("plugin kv key", key)?;
        run!(&self.exec, |e| sqlx::query(
            "INSERT INTO plugin_kv (plugin, key, value, updated_at) VALUES ($1, $2, $3, $4) \
             ON CONFLICT (plugin, key) DO UPDATE SET value = excluded.value, \
             updated_at = excluded.updated_at"
        )
        .bind(plugin)
        .bind(key)
        .bind(value)
        .bind(DateTime::<Utc>::from(Timestamp::now()))
        .execute(e))
        .map(|_| ())
    }

    async fn delete(&self, plugin: &str, key: &str) -> Result<(), StoreError> {
        validate_text("plugin kv plugin", plugin)?;
        validate_text("plugin kv key", key)?;
        run!(&self.exec, |e| sqlx::query(
            "DELETE FROM plugin_kv WHERE plugin = $1 AND key = $2"
        )
        .bind(plugin)
        .bind(key)
        .execute(e))
        .map(|_| ())
    }

    async fn list_keys(
        &self,
        plugin: &str,
        prefix: &str,
        limit: u32,
    ) -> Result<Vec<String>, StoreError> {
        validate_text("plugin kv plugin", plugin)?;
        validate_text("plugin kv prefix", prefix)?;
        let limit = check_limit(limit)?;
        // starts_with needs no LIKE escaping.
        run!(&self.exec, |e| sqlx::query_scalar(
            "SELECT key FROM plugin_kv WHERE plugin = $1 AND starts_with(key, $2) \
             ORDER BY key COLLATE \"C\" LIMIT $3"
        )
        .bind(plugin)
        .bind(prefix)
        .bind(i64::from(limit))
        .fetch_all(e))
    }

    async fn usage(&self, plugin: &str) -> Result<(u64, u64), StoreError> {
        validate_text("plugin kv plugin", plugin)?;
        let (bytes, keys): (i64, i64) = run!(&self.exec, |e| {
            sqlx::query_as(
            "SELECT COALESCE(SUM(octet_length(value)), 0)::bigint, COUNT(*) FROM plugin_kv WHERE plugin = $1"
        )
        .bind(plugin)
        .fetch_one(e)
        })?;
        let conv = |n: i64| {
            u64::try_from(n).map_err(|_| StoreError::Database(format!("corrupt usage {n}")))
        };
        Ok((conv(bytes)?, conv(keys)?))
    }

    async fn delete_all(&self, plugin: &str, limit: u32) -> Result<u64, StoreError> {
        validate_text("plugin kv plugin", plugin)?;
        let limit = check_prune_limit(limit)?;
        let done = run!(&self.exec, |e| sqlx::query(
            "DELETE FROM plugin_kv WHERE plugin = $1 AND key IN \
             (SELECT key FROM plugin_kv WHERE plugin = $1 LIMIT $2)"
        )
        .bind(plugin)
        .bind(limit)
        .execute(e))?;
        Ok(done.rows_affected())
    }

    async fn set_with_quota(
        &self,
        plugin: &str,
        key: &str,
        value: &[u8],
        max_bytes: u64,
        max_keys: u64,
    ) -> Result<bool, StoreError> {
        validate_text("plugin kv plugin", plugin)?;
        validate_text("plugin kv key", key)?;
        // READ COMMITTED does not serialize read-modify-write, so each plugin's quota
        // writes queue on a transaction-scoped advisory lock (class `KV_LOCK_CLASS`, held
        // until the enclosing transaction ends, so the next holder's statement sees this
        // commit). Then one conditional statement (P2) writes only if the totals, with
        // this key's old value replaced by the new one, stay within quota.
        atomic!(&self.exec, |conn| async move {
            sqlx::query("SELECT pg_advisory_xact_lock($1::int4, hashtext($2::text))")
                .bind(KV_LOCK_CLASS)
                .bind(plugin)
                .execute(&mut *conn)
                .await
                .map_err(map_db_err)?;
            let done = sqlx::query(
                "INSERT INTO plugin_kv (plugin, key, value, updated_at) \
                 SELECT $1::text, $2::text, $3::bytea, $4::timestamptz \
                 WHERE (SELECT COALESCE(SUM(octet_length(value)), 0) FROM plugin_kv \
                        WHERE plugin = $1 AND key <> $2) + octet_length($3::bytea) <= $5::bigint \
                   AND (SELECT COUNT(*) FROM plugin_kv WHERE plugin = $1 AND key <> $2) + 1 <= $6::bigint \
                 ON CONFLICT (plugin, key) DO UPDATE SET value = excluded.value, \
                 updated_at = excluded.updated_at",
            )
            .bind(plugin)
            .bind(key)
            .bind(value)
            .bind(DateTime::<Utc>::from(Timestamp::now()))
            .bind(i64::try_from(max_bytes).unwrap_or(i64::MAX))
            .bind(i64::try_from(max_keys).unwrap_or(i64::MAX))
            .execute(&mut *conn)
            .await
            .map_err(map_db_err)?;
            Ok(done.rows_affected() == 1)
        })
    }
}

#[async_trait]
impl ThemeRepo for PgRepos {
    async fn upsert(&self, theme: &InstalledTheme) -> Result<(), StoreError> {
        validate_text("theme name", &theme.name)?;
        validate_text("theme version", &theme.version)?;
        validate_json(&theme.manifest)?;
        // `active` is written on insert only; a replaced theme keeps its state and
        // activation goes through `activate`.
        run!(&self.exec, |e| sqlx::query(
            "INSERT INTO themes (name, version, artifact_hash, manifest, active, installed_at) \
             VALUES ($1, $2, $3, $4, $5, $6) \
             ON CONFLICT (name) DO UPDATE SET version = excluded.version, \
             artifact_hash = excluded.artifact_hash, manifest = excluded.manifest"
        )
        .bind(&theme.name)
        .bind(&theme.version)
        .bind(theme.artifact_hash.as_str())
        .bind(Json(&theme.manifest))
        .bind(theme.active)
        .bind(DateTime::<Utc>::from(theme.installed_at))
        .execute(e))
        .map(|_| ())
    }

    async fn get(&self, id: &str) -> Result<Option<InstalledTheme>, StoreError> {
        validate_text("theme name", id)?;
        let row = run!(&self.exec, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {THEME_COLUMNS} FROM themes WHERE name = $1"
        )))
        .bind(id)
        .fetch_optional(e))?;
        row.as_ref().map(codec::theme).transpose()
    }

    async fn list(&self) -> Result<Vec<InstalledTheme>, StoreError> {
        let rows = run!(&self.exec, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {THEME_COLUMNS} FROM themes ORDER BY name COLLATE \"C\""
        )))
        .fetch_all(e))?;
        rows.iter().map(codec::theme).collect()
    }

    async fn active(&self) -> Result<Option<InstalledTheme>, StoreError> {
        let row = run!(&self.exec, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {THEME_COLUMNS} FROM themes WHERE active"
        )))
        .fetch_optional(e))?;
        row.as_ref().map(codec::theme).transpose()
    }

    async fn activate(&self, id: &str) -> Result<(), StoreError> {
        validate_text("theme name", id)?;
        atomic!(&self.exec, |conn| async {
            sqlx::query("UPDATE themes SET active = false WHERE active")
                .execute(&mut *conn)
                .await
                .map_err(crate::exec::map_db_err)?;
            let done = sqlx::query("UPDATE themes SET active = true WHERE name = $1")
                .bind(id)
                .execute(&mut *conn)
                .await
                .map_err(crate::exec::map_db_err)?;
            if done.rows_affected() == 0 {
                return Err(StoreError::NotFound);
            }
            Ok(())
        })
    }
}
