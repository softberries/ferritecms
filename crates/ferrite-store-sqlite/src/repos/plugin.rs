//! Artifacts, media, installed plugins, plugin key-value storage and themes.

use async_trait::async_trait;
use ferrite_domain::{
    Artifact, ArtifactHash, ContentId, InstalledPlugin, InstalledTheme, Media, Timestamp,
};
use ferrite_store::{
    ArtifactRepo, MediaRepo, PluginKvRepo, PluginRepo, StoreError, ThemeRepo, check_limit,
    validate_json, validate_text,
};

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
        run!(&self.writer, |e| sqlx::query(
            "INSERT INTO media (item_id, artifact_hash, mime, bytes, width, height, alt, \
             original_name, variants) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(media.item_id.as_uuid())
        .bind(media.artifact_hash.as_str())
        .bind(&media.mime)
        .bind(media.bytes)
        .bind(media.width)
        .bind(media.height)
        .bind(&media.alt)
        .bind(&media.original_name)
        .bind(codec::json_to_db(&media.variants)?)
        .execute(e))
        .map(|_| ())
    }

    async fn get(&self, id: ContentId) -> Result<Option<Media>, StoreError> {
        let row = run!(&self.reader, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {MEDIA_COLUMNS} FROM media WHERE item_id = ?"
        )))
        .bind(id.as_uuid())
        .fetch_optional(e))?;
        row.as_ref().map(codec::media).transpose()
    }

    async fn update(&self, media: &Media) -> Result<(), StoreError> {
        validate_text("media mime", &media.mime)?;
        validate_text("media alt", &media.alt)?;
        validate_json(&media.variants)?;
        let done = run!(&self.writer, |e| {
            sqlx::query(
            "UPDATE media SET mime = ?, bytes = ?, width = ?, height = ?, alt = ?, variants = ? \
             WHERE item_id = ?"
        )
        .bind(&media.mime)
        .bind(media.bytes)
        .bind(media.width)
        .bind(media.height)
        .bind(&media.alt)
        .bind(codec::json_to_db(&media.variants)?)
        .bind(media.item_id.as_uuid())
        .execute(e)
        })?;
        if done.rows_affected() == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }
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
        let row = run!(&self.reader, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {PLUGIN_COLUMNS} FROM plugins WHERE name = ?"
        )))
        .bind(id)
        .fetch_optional(e))?;
        row.as_ref().map(codec::plugin).transpose()
    }

    async fn list(&self) -> Result<Vec<InstalledPlugin>, StoreError> {
        let rows = run!(&self.reader, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {PLUGIN_COLUMNS} FROM plugins ORDER BY name"
        )))
        .fetch_all(e))?;
        rows.iter().map(codec::plugin).collect()
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
}

#[async_trait]
impl PluginKvRepo for SqliteRepos {
    async fn get(&self, plugin: &str, key: &str) -> Result<Option<Vec<u8>>, StoreError> {
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
