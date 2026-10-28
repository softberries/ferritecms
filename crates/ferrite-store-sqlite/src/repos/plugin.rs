//! Artifacts, media, installed plugins, plugin key-value storage and themes.

use std::collections::HashMap;

use async_trait::async_trait;
use ferrite_domain::{
    Artifact, ArtifactHash, ContentId, InstalledPlugin, InstalledTheme, Media, Timestamp,
};
use ferrite_store::{
    ArtifactRepo, MAX_ID_FILTER, MediaRepo, PluginKvRepo, PluginRepo, RetainedPlugin, StoreError,
    ThemeRepo, check_len, check_limit, check_prune_limit, retained_marker, validate_json,
    validate_text,
};

use sqlx::{QueryBuilder, Row, Sqlite};

use super::SqliteRepos;
use crate::{
    codec::{self, ARTIFACT_COLUMNS, MEDIA_COLUMNS, PLUGIN_COLUMNS, THEME_COLUMNS},
    exec::{atomic, run},
};

#[async_trait]
impl ArtifactRepo for SqliteRepos {
    async fn insert_if_absent(&self, artifact: &Artifact) -> Result<bool, StoreError> {
        validate_text("artifact mime", &artifact.mime)?;
        let done = run!(&self.writer, |e| {
            sqlx::query(
            "INSERT INTO artifacts (hash, kind, bytes, mime, created_at) VALUES (?, ?, ?, ?, ?) \
             ON CONFLICT DO NOTHING"
        )
        .bind(artifact.hash.as_str())
        .bind(artifact.kind.as_str())
        .bind(artifact.bytes)
        .bind(&artifact.mime)
        .bind(artifact.created_at.as_micros())
        .execute(e)
        })?;
        Ok(done.rows_affected() == 1)
    }

    async fn get(&self, hash: &ArtifactHash) -> Result<Option<Artifact>, StoreError> {
        let row = run!(&self.reader, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {ARTIFACT_COLUMNS} FROM artifacts WHERE hash = ?"
        )))
        .bind(hash.as_str())
        .fetch_optional(e))?;
        row.as_ref().map(codec::artifact).transpose()
    }

    async fn delete(&self, hash: &ArtifactHash) -> Result<(), StoreError> {
        run!(&self.writer, |e| sqlx::query(
            "DELETE FROM artifacts WHERE hash = ?"
        )
        .bind(hash.as_str())
        .execute(e))
        .map(|_| ())
    }
}

#[async_trait]
impl MediaRepo for SqliteRepos {
    async fn insert(&self, media: &Media) -> Result<(), StoreError> {
        validate_text("media mime", &media.mime)?;
        validate_text("media alt", &media.alt)?;
        validate_text("media original_name", &media.original_name)?;
        validate_json(&media.variants)?;
        let variants = codec::json_to_db(&media.variants)?;
        // The row and its `media_variants` rows commit together.
        atomic!(&self.writer, |conn| async {
            sqlx::query(
                "INSERT INTO media (item_id, artifact_hash, mime, bytes, width, height, alt, \
                 original_name, variants, sanitized) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
        let row = run!(&self.reader, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {MEDIA_COLUMNS} FROM media WHERE item_id = ?"
        )))
        .bind(id.as_uuid())
        .fetch_optional(e))?;
        row.as_ref().map(codec::media).transpose()
    }

    async fn get_for_update(&self, id: ContentId) -> Result<Option<Media>, StoreError> {
        // Inside a Tx the reader is the Tx's connection, which holds the single writer:
        // no other write can touch the row until the Tx ends.
        MediaRepo::get(self, id).await
    }

    async fn update(&self, media: &Media) -> Result<(), StoreError> {
        validate_text("media mime", &media.mime)?;
        validate_text("media alt", &media.alt)?;
        validate_json(&media.variants)?;
        let variants = codec::json_to_db(&media.variants)?;
        atomic!(&self.writer, |conn| async {
            let done = sqlx::query(
                "UPDATE media SET mime = ?, bytes = ?, width = ?, height = ?, \
                 alt = ?, variants = ?, sanitized = ? WHERE item_id = ?",
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
            sqlx::query("DELETE FROM media_variants WHERE item_id = ?")
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
        let mut qb = QueryBuilder::<Sqlite>::new(format!(
            "SELECT {MEDIA_COLUMNS} FROM media WHERE item_id IN ("
        ));
        let mut list = qb.separated(", ");
        for id in ids {
            list.push_bind(id.as_uuid());
        }
        list.push_unseparated(")");
        let rows = run!(&self.reader, |e| qb.build().fetch_all(e))?;
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
        let rows = run!(&self.reader, |e| sqlx::query(FIND_MEDIA_BY_ARTIFACT_SQL)
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
WHERE EXISTS (SELECT 1 FROM artifacts WHERE hash = ?1 AND kind IN ('media', 'variant')) \
AND item_id IN (SELECT item_id FROM media WHERE artifact_hash = ?1 \
UNION SELECT item_id FROM media_variants WHERE hash = ?1) \
ORDER BY item_id LIMIT ?2";

/// Rebuilds the `media_variants` rows of `item` from its `variants` JSON (the rule of
/// migration 0003's backfill): one row per object value with a string `hash`.
const SYNC_VARIANTS_SQL: &str = "INSERT INTO media_variants (hash, item_id, name) \
SELECT json_extract(v.value, '$.hash'), m.item_id, v.key \
FROM media AS m, json_each(m.variants) AS v \
WHERE m.item_id = ?1 AND json_type(m.variants) = 'object' AND v.type = 'object' \
AND json_type(v.value, '$.hash') = 'text'";

async fn sync_variants(
    conn: &mut sqlx::SqliteConnection,
    item: ContentId,
) -> Result<(), StoreError> {
    sqlx::query(SYNC_VARIANTS_SQL)
        .bind(item.as_uuid())
        .execute(conn)
        .await
        .map_err(crate::exec::map_db_err)?;
    Ok(())
}

#[async_trait]
impl PluginRepo for SqliteRepos {
    async fn upsert(&self, plugin: &InstalledPlugin) -> Result<(), StoreError> {
        validate_text("plugin name", &plugin.name)?;
        validate_text("plugin version", &plugin.version)?;
        if let Some(reason) = &plugin.disabled_reason {
            validate_text("plugin disabled_reason", reason)?;
        }
        validate_json(&plugin.manifest)?;
        validate_json(&plugin.granted)?;
        // A replaced plugin keeps its original install time.
        run!(&self.writer, |e| sqlx::query(
            "INSERT INTO plugins (name, version, artifact_hash, manifest, granted, enabled, \
             disabled_reason, installed_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT (name) DO UPDATE SET version = excluded.version, \
             artifact_hash = excluded.artifact_hash, manifest = excluded.manifest, \
             granted = excluded.granted, enabled = excluded.enabled, \
             disabled_reason = excluded.disabled_reason, updated_at = excluded.updated_at"
        )
        .bind(&plugin.name)
        .bind(&plugin.version)
        .bind(plugin.artifact_hash.as_str())
        .bind(codec::json_to_db(&plugin.manifest)?)
        .bind(codec::json_to_db(&plugin.granted)?)
        .bind(plugin.enabled)
        .bind(plugin.disabled_reason.as_deref())
        .bind(plugin.installed_at.as_micros())
        .bind(plugin.updated_at.as_micros())
        .execute(e))
        .map(|_| ())
    }

    async fn get(&self, id: &str) -> Result<Option<InstalledPlugin>, StoreError> {
        validate_text("plugin name", id)?;
        let row = run!(&self.reader, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {PLUGIN_COLUMNS} FROM plugins WHERE name = ?"
        )))
        .bind(id)
        .fetch_optional(e))?;
        row.as_ref().map(codec::plugin).transpose()
    }

    async fn is_enabled(&self, id: &str) -> Result<Option<bool>, StoreError> {
        validate_text("plugin name", id)?;
        let row = run!(&self.reader, |e| sqlx::query(
            "SELECT enabled FROM plugins WHERE name = ?"
        )
        .bind(id)
        .fetch_optional(e))?;
        row.map(|r| r.try_get::<bool, _>("enabled").map_err(crate::map_db_err))
            .transpose()
    }

    async fn get_for_update(&self, id: &str) -> Result<Option<InstalledPlugin>, StoreError> {
        // Inside a Tx the reader is the Tx's connection, which holds the single writer:
        // no other write can touch the row until the Tx ends.
        PluginRepo::get(self, id).await
    }

    async fn list(&self) -> Result<Vec<InstalledPlugin>, StoreError> {
        let rows = run!(&self.reader, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {PLUGIN_COLUMNS} FROM plugins ORDER BY name"
        )))
        .fetch_all(e))?;
        rows.iter().map(codec::plugin).collect()
    }

    async fn action_subscribers(&self, hook: &str) -> Result<Vec<String>, StoreError> {
        validate_text("hook", hook)?;
        // `json_each` compares the key as text, so hook names with dots need no quoting.
        let rows = run!(&self.reader, |e| sqlx::query(
            "SELECT name FROM plugins WHERE enabled = 1 \
             AND json_type(manifest, '$.hooks') = 'object' \
             AND EXISTS (SELECT 1 FROM json_each(manifest, '$.hooks') WHERE key = ?) \
             ORDER BY name"
        )
        .bind(hook)
        .fetch_all(e))?;
        rows.iter()
            .map(|r| r.try_get::<String, _>("name").map_err(crate::map_db_err))
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
        let done = run!(&self.writer, |e| sqlx::query(
            "UPDATE plugins SET enabled = ?, disabled_reason = ?, updated_at = ? WHERE name = ?"
        )
        .bind(enabled)
        .bind(reason)
        .bind(Timestamp::now().as_micros())
        .bind(id)
        .execute(e))?;
        if done.rows_affected() == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    async fn delete(&self, id: &str) -> Result<(), StoreError> {
        validate_text("plugin name", id)?;
        run!(&self.writer, |e| sqlx::query(
            "DELETE FROM plugins WHERE name = ?"
        )
        .bind(id)
        .execute(e))
        .map(|_| ())
    }

    async fn retained(&self) -> Result<Vec<RetainedPlugin>, StoreError> {
        // `plugin.` is 7 characters and `.deleted_at` 11: the name is what lies between.
        let rows = run!(&self.reader, |e| sqlx::query(
            "SELECT o.key, o.value FROM options AS o \
             WHERE substr(o.key, 1, 7) = 'plugin.' AND substr(o.key, -11) = '.deleted_at' \
             AND length(o.key) > 18 AND NOT EXISTS (SELECT 1 FROM plugins AS p \
             WHERE p.name = substr(o.key, 8, length(o.key) - 18))"
        )
        .fetch_all(e))?;
        let mut found = Vec::with_capacity(rows.len());
        for row in &rows {
            let key: String = row.try_get("key").map_err(crate::map_db_err)?;
            let value: String = row.try_get("value").map_err(crate::map_db_err)?;
            if let Some(r) = retained_marker(&key, &codec::json_from_db(&value)?) {
                found.push(r);
            }
        }
        found.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(found)
    }

    async fn auto_disable(
        &self,
        id: &str,
        reason: &str,
        observed_updated_at: Timestamp,
    ) -> Result<bool, StoreError> {
        validate_text("plugin name", id)?;
        validate_text("plugin disabled_reason", reason)?;
        // The process clock is the database clock on SQLite.
        let done = run!(&self.writer, |e| sqlx::query(
            "UPDATE plugins SET enabled = 0, disabled_reason = ?, updated_at = ? \
             WHERE name = ? AND enabled = 1 AND updated_at = ?"
        )
        .bind(reason)
        .bind(Timestamp::now().as_micros())
        .bind(id)
        .bind(observed_updated_at.as_micros())
        .execute(e))?;
        Ok(done.rows_affected() == 1)
    }
}

#[async_trait]
impl PluginKvRepo for SqliteRepos {
    async fn get(&self, plugin: &str, key: &str) -> Result<Option<Vec<u8>>, StoreError> {
        validate_text("plugin kv plugin", plugin)?;
        validate_text("plugin kv key", key)?;
        run!(&self.reader, |e| sqlx::query_scalar(
            "SELECT value FROM plugin_kv WHERE plugin = ? AND key = ?"
        )
        .bind(plugin)
        .bind(key)
        .fetch_optional(e))
    }

    async fn set(&self, plugin: &str, key: &str, value: &[u8]) -> Result<(), StoreError> {
        validate_text("plugin kv plugin", plugin)?;
        validate_text("plugin kv key", key)?;
        run!(&self.writer, |e| sqlx::query(
            "INSERT INTO plugin_kv (plugin, key, value, updated_at) VALUES (?, ?, ?, ?) \
             ON CONFLICT (plugin, key) DO UPDATE SET value = excluded.value, \
             updated_at = excluded.updated_at"
        )
        .bind(plugin)
        .bind(key)
        .bind(value)
        .bind(Timestamp::now().as_micros())
        .execute(e))
        .map(|_| ())
    }

    async fn delete(&self, plugin: &str, key: &str) -> Result<(), StoreError> {
        validate_text("plugin kv plugin", plugin)?;
        validate_text("plugin kv key", key)?;
        run!(&self.writer, |e| sqlx::query(
            "DELETE FROM plugin_kv WHERE plugin = ? AND key = ?"
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
        // substr/length compare by characters, so the prefix match needs no LIKE escaping.
        run!(&self.reader, |e| sqlx::query_scalar(
            "SELECT key FROM plugin_kv WHERE plugin = ?1 AND substr(key, 1, length(?2)) = ?2 \
             ORDER BY key LIMIT ?3"
        )
        .bind(plugin)
        .bind(prefix)
        .bind(i64::from(limit))
        .fetch_all(e))
    }

    async fn usage(&self, plugin: &str) -> Result<(u64, u64), StoreError> {
        validate_text("plugin kv plugin", plugin)?;
        let (bytes, keys): (i64, i64) = run!(&self.reader, |e| sqlx::query_as(
            "SELECT COALESCE(SUM(length(value)), 0), COUNT(*) FROM plugin_kv WHERE plugin = ?"
        )
        .bind(plugin)
        .fetch_one(e))?;
        let conv = |n: i64| {
            u64::try_from(n).map_err(|_| StoreError::Database(format!("corrupt usage {n}")))
        };
        Ok((conv(bytes)?, conv(keys)?))
    }

    async fn delete_all(&self, plugin: &str, limit: u32) -> Result<u64, StoreError> {
        validate_text("plugin kv plugin", plugin)?;
        let limit = check_prune_limit(limit)?;
        let done = run!(&self.writer, |e| sqlx::query(
            "DELETE FROM plugin_kv WHERE rowid IN \
             (SELECT rowid FROM plugin_kv WHERE plugin = ? LIMIT ?)"
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
        // One statement (P2): the INSERT happens only if the totals, with this key's old
        // value replaced by the new one, stay within quota; the single writer serializes
        // concurrent callers. Bytes are the blob length, as in `usage`.
        let done = run!(&self.writer, |e| sqlx::query(
            "INSERT INTO plugin_kv (plugin, key, value, updated_at) \
             SELECT ?1, ?2, ?3, ?4 \
             WHERE (SELECT COALESCE(SUM(length(value)), 0) FROM plugin_kv \
                    WHERE plugin = ?1 AND key <> ?2) + length(?3) <= ?5 \
               AND (SELECT COUNT(*) FROM plugin_kv WHERE plugin = ?1 AND key <> ?2) + 1 <= ?6 \
             ON CONFLICT (plugin, key) DO UPDATE SET value = excluded.value, \
             updated_at = excluded.updated_at"
        )
        .bind(plugin)
        .bind(key)
        .bind(value)
        .bind(Timestamp::now().as_micros())
        .bind(i64::try_from(max_bytes).unwrap_or(i64::MAX))
        .bind(i64::try_from(max_keys).unwrap_or(i64::MAX))
        .execute(e))?;
        Ok(done.rows_affected() == 1)
    }
}

#[async_trait]
impl ThemeRepo for SqliteRepos {
    async fn upsert(&self, theme: &InstalledTheme) -> Result<(), StoreError> {
        validate_text("theme name", &theme.name)?;
        validate_text("theme version", &theme.version)?;
        validate_json(&theme.manifest)?;
        // `active` is written on insert only; a replaced theme keeps its state and
        // activation goes through `activate`.
        run!(&self.writer, |e| sqlx::query(
            "INSERT INTO themes (name, version, artifact_hash, manifest, active, installed_at) \
             VALUES (?, ?, ?, ?, ?, ?) \
             ON CONFLICT (name) DO UPDATE SET version = excluded.version, \
             artifact_hash = excluded.artifact_hash, manifest = excluded.manifest"
        )
        .bind(&theme.name)
        .bind(&theme.version)
        .bind(theme.artifact_hash.as_str())
        .bind(codec::json_to_db(&theme.manifest)?)
        .bind(theme.active)
        .bind(theme.installed_at.as_micros())
        .execute(e))
        .map(|_| ())
    }

    async fn get(&self, id: &str) -> Result<Option<InstalledTheme>, StoreError> {
        validate_text("theme name", id)?;
        let row = run!(&self.reader, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {THEME_COLUMNS} FROM themes WHERE name = ?"
        )))
        .bind(id)
        .fetch_optional(e))?;
        row.as_ref().map(codec::theme).transpose()
    }

    async fn list(&self) -> Result<Vec<InstalledTheme>, StoreError> {
        let rows = run!(&self.reader, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {THEME_COLUMNS} FROM themes ORDER BY name"
        )))
        .fetch_all(e))?;
        rows.iter().map(codec::theme).collect()
    }

    async fn active(&self) -> Result<Option<InstalledTheme>, StoreError> {
        let row = run!(&self.reader, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {THEME_COLUMNS} FROM themes WHERE active = 1"
        )))
        .fetch_optional(e))?;
        row.as_ref().map(codec::theme).transpose()
    }

    async fn activate(&self, id: &str) -> Result<(), StoreError> {
        validate_text("theme name", id)?;
        atomic!(&self.writer, |conn| async {
            sqlx::query("UPDATE themes SET active = 0 WHERE active = 1")
                .execute(&mut *conn)
                .await
                .map_err(crate::exec::map_db_err)?;
            let done = sqlx::query("UPDATE themes SET active = 1 WHERE name = ?")
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
