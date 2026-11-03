//! Plugin trust (`0015_plugin_publishers`, M6-2) on SQLite: publisher keys, owners and
//! the registry cache.

use async_trait::async_trait;
use ferrite_domain::Timestamp;
use ferrite_domain::plugin_trust::{
    InstallSource, KeyId, KeySource, OwnerId, PluginOwner, PublisherKey, RegistryCacheRow,
    Revocation, TrustedKey,
};
use ferrite_store::{
    PluginTrustRepo, StoreError, check_plugin_owner, check_registry_row, check_trusted_key,
    validate_text,
};
use sqlx::sqlite::SqliteRow;
use sqlx::{AssertSqlSafe, Row};

use super::SqliteRepos;
use crate::exec::{map_db_err, run};

const KEY_COLUMNS: &str =
    "key_id, public_key, label, source, trusted_at, revoked_at, revoked_reason, compromised";
const OWNER_COLUMNS: &str = "plugin, owner, signer_key, signer_public_key, signature, publisher, source, \
     registry_url, bound_at, updated_at";
const REGISTRY_COLUMNS: &str = "url, serial, index_json, signature, generated_at, fetched_at, \
     checked_at, last_error, last_error_at";

fn corrupt(what: &str, value: &str) -> StoreError {
    StoreError::Database(format!("corrupt {what} {value}"))
}

fn ts(micros: i64) -> Result<Timestamp, StoreError> {
    Ok(Timestamp::from_micros(micros)?)
}

fn get<'r, T: sqlx::Decode<'r, sqlx::Sqlite> + sqlx::Type<sqlx::Sqlite>>(
    row: &'r SqliteRow,
    col: &str,
) -> Result<T, StoreError> {
    row.try_get(col).map_err(map_db_err)
}

fn key_row(row: &SqliteRow) -> Result<TrustedKey, StoreError> {
    let id: String = get(row, "key_id")?;
    let b64: String = get(row, "public_key")?;
    let key = PublisherKey::from_base64(&b64).map_err(|_| corrupt("publisher key", &id))?;
    if key.key_id.to_string() != id {
        return Err(corrupt("publisher key id", &id));
    }
    let source: String = get(row, "source")?;
    let revoked_at: Option<i64> = get(row, "revoked_at")?;
    let revoked = match revoked_at {
        None => None,
        Some(at) => Some(Revocation {
            at: ts(at)?,
            reason: get::<Option<String>>(row, "revoked_reason")?.unwrap_or_default(),
            compromised: get(row, "compromised")?,
        }),
    };
    Ok(TrustedKey {
        key,
        label: get(row, "label")?,
        source: KeySource::parse(&source).ok_or_else(|| corrupt("key source", &source))?,
        trusted_at: ts(get(row, "trusted_at")?)?,
        revoked,
    })
}

fn owner_row(row: &SqliteRow) -> Result<PluginOwner, StoreError> {
    let owner: Option<String> = get(row, "owner")?;
    let signer: Option<String> = get(row, "signer_key")?;
    let source: String = get(row, "source")?;
    Ok(PluginOwner {
        plugin: get(row, "plugin")?,
        owner: owner
            .map(|o| {
                o.parse::<OwnerId>()
                    .map_err(|_| corrupt("plugin owner", &o))
            })
            .transpose()?,
        signer_key: signer
            .map(|k| KeyId::parse(&k).map_err(|_| corrupt("signer key", &k)))
            .transpose()?,
        signer_public_key: get(row, "signer_public_key")?,
        signature: get(row, "signature")?,
        publisher: get(row, "publisher")?,
        source: InstallSource::parse(&source).ok_or_else(|| corrupt("install source", &source))?,
        registry_url: get(row, "registry_url")?,
        bound_at: ts(get(row, "bound_at")?)?,
        updated_at: ts(get(row, "updated_at")?)?,
    })
}

fn registry_row(row: &SqliteRow) -> Result<RegistryCacheRow, StoreError> {
    let err_at: Option<i64> = get(row, "last_error_at")?;
    Ok(RegistryCacheRow {
        url: get(row, "url")?,
        serial: get(row, "serial")?,
        index_json: get(row, "index_json")?,
        signature: get(row, "signature")?,
        generated_at: ts(get(row, "generated_at")?)?,
        fetched_at: ts(get(row, "fetched_at")?)?,
        checked_at: ts(get(row, "checked_at")?)?,
        last_error: get(row, "last_error")?,
        last_error_at: err_at.map(ts).transpose()?,
    })
}

#[async_trait]
impl PluginTrustRepo for SqliteRepos {
    async fn get_key(&self, key_id: &KeyId) -> Result<Option<TrustedKey>, StoreError> {
        let id = key_id.to_string();
        let row = run!(&self.reader, |e| sqlx::query(AssertSqlSafe(format!(
            "SELECT {KEY_COLUMNS} FROM plugin_publishers WHERE key_id = ?"
        )))
        .bind(&id)
        .fetch_optional(e))?;
        row.as_ref().map(key_row).transpose()
    }

    async fn list_keys(&self) -> Result<Vec<TrustedKey>, StoreError> {
        let rows = run!(&self.reader, |e| sqlx::query(AssertSqlSafe(format!(
            "SELECT {KEY_COLUMNS} FROM plugin_publishers ORDER BY key_id"
        )))
        .fetch_all(e))?;
        rows.iter().map(key_row).collect()
    }

    async fn upsert_key(&self, key: &TrustedKey) -> Result<(), StoreError> {
        check_trusted_key(key)?;
        let r = key.revoked.as_ref();
        run!(&self.writer, |e| sqlx::query(
            "INSERT INTO plugin_publishers (key_id, public_key, label, source, trusted_at, \
             revoked_at, revoked_reason, compromised) VALUES (?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT (key_id) DO UPDATE SET public_key = excluded.public_key, \
             label = excluded.label, source = excluded.source, \
             trusted_at = excluded.trusted_at, revoked_at = excluded.revoked_at, \
             revoked_reason = excluded.revoked_reason, compromised = excluded.compromised"
        )
        .bind(key.key.key_id.to_string())
        .bind(&key.key.base64)
        .bind(&key.label)
        .bind(key.source.as_str())
        .bind(key.trusted_at.as_micros())
        .bind(r.map(|r| r.at.as_micros()))
        .bind(r.map(|r| r.reason.as_str()))
        .bind(r.is_some_and(|r| r.compromised))
        .execute(e))?;
        Ok(())
    }

    async fn delete_key(&self, key_id: &KeyId) -> Result<bool, StoreError> {
        let done = run!(&self.writer, |e| sqlx::query(
            "DELETE FROM plugin_publishers WHERE key_id = ?"
        )
        .bind(key_id.to_string())
        .execute(e))?;
        Ok(done.rows_affected() > 0)
    }

    async fn revoke_key(
        &self,
        key_id: &KeyId,
        revocation: &Revocation,
    ) -> Result<bool, StoreError> {
        validate_text("revocation reason", &revocation.reason)?;
        let done = run!(&self.writer, |e| sqlx::query(
            "UPDATE plugin_publishers SET revoked_at = ?, revoked_reason = ?, compromised = ? \
             WHERE key_id = ?"
        )
        .bind(revocation.at.as_micros())
        .bind(&revocation.reason)
        .bind(revocation.compromised)
        .bind(key_id.to_string())
        .execute(e))?;
        Ok(done.rows_affected() > 0)
    }

    async fn get_owner(&self, plugin: &str) -> Result<Option<PluginOwner>, StoreError> {
        validate_text("plugin owner plugin", plugin)?;
        let row = run!(&self.reader, |e| sqlx::query(AssertSqlSafe(format!(
            "SELECT {OWNER_COLUMNS} FROM plugin_owners WHERE plugin = ?"
        )))
        .bind(plugin)
        .fetch_optional(e))?;
        row.as_ref().map(owner_row).transpose()
    }

    async fn list_owners(&self) -> Result<Vec<PluginOwner>, StoreError> {
        let rows = run!(&self.reader, |e| sqlx::query(AssertSqlSafe(format!(
            "SELECT {OWNER_COLUMNS} FROM plugin_owners ORDER BY plugin"
        )))
        .fetch_all(e))?;
        rows.iter().map(owner_row).collect()
    }

    async fn put_owner(&self, owner: &PluginOwner) -> Result<(), StoreError> {
        check_plugin_owner(owner)?;
        run!(&self.writer, |e| sqlx::query(
            "INSERT INTO plugin_owners (plugin, owner, signer_key, signer_public_key, signature, \
             publisher, source, registry_url, bound_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT (plugin) DO UPDATE SET owner = excluded.owner, \
             signer_key = excluded.signer_key, signer_public_key = excluded.signer_public_key, \
             signature = excluded.signature, \
             publisher = excluded.publisher, source = excluded.source, \
             registry_url = excluded.registry_url, updated_at = excluded.updated_at"
        )
        .bind(&owner.plugin)
        .bind(owner.owner.as_ref().map(ToString::to_string))
        .bind(owner.signer_key.map(|k| k.to_string()))
        .bind(owner.signer_public_key.as_deref())
        .bind(owner.signature.as_deref())
        .bind(&owner.publisher)
        .bind(owner.source.as_str())
        .bind(owner.registry_url.as_deref())
        .bind(owner.bound_at.as_micros())
        .bind(owner.updated_at.as_micros())
        .execute(e))?;
        Ok(())
    }

    async fn delete_owner(&self, plugin: &str) -> Result<bool, StoreError> {
        validate_text("plugin owner plugin", plugin)?;
        let done = run!(&self.writer, |e| sqlx::query(
            "DELETE FROM plugin_owners WHERE plugin = ?"
        )
        .bind(plugin)
        .execute(e))?;
        Ok(done.rows_affected() > 0)
    }

    async fn owners_signed_by(&self, key_id: &KeyId) -> Result<Vec<PluginOwner>, StoreError> {
        let id = key_id.to_string();
        let rows = run!(&self.reader, |e| sqlx::query(AssertSqlSafe(format!(
            "SELECT {OWNER_COLUMNS} FROM plugin_owners WHERE signer_key = ? ORDER BY plugin"
        )))
        .bind(&id)
        .fetch_all(e))?;
        rows.iter().map(owner_row).collect()
    }

    async fn get_registry(&self, url: &str) -> Result<Option<RegistryCacheRow>, StoreError> {
        validate_text("registry url", url)?;
        let row = run!(&self.reader, |e| sqlx::query(AssertSqlSafe(format!(
            "SELECT {REGISTRY_COLUMNS} FROM plugin_registry WHERE url = ?"
        )))
        .bind(url)
        .fetch_optional(e))?;
        row.as_ref().map(registry_row).transpose()
    }

    async fn put_registry(&self, row: &RegistryCacheRow) -> Result<(), StoreError> {
        check_registry_row(row)?;
        run!(&self.writer, |e| sqlx::query(
            "INSERT INTO plugin_registry (url, serial, index_json, signature, generated_at, \
             fetched_at, checked_at, last_error, last_error_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT (url) DO UPDATE SET serial = excluded.serial, \
             index_json = excluded.index_json, signature = excluded.signature, \
             generated_at = excluded.generated_at, fetched_at = excluded.fetched_at, \
             checked_at = excluded.checked_at, last_error = excluded.last_error, \
             last_error_at = excluded.last_error_at"
        )
        .bind(&row.url)
        .bind(row.serial)
        .bind(&row.index_json)
        .bind(&row.signature)
        .bind(row.generated_at.as_micros())
        .bind(row.fetched_at.as_micros())
        .bind(row.checked_at.as_micros())
        .bind(row.last_error.as_deref())
        .bind(row.last_error_at.map(|t| t.as_micros()))
        .execute(e))?;
        Ok(())
    }

    async fn registry_checked(
        &self,
        url: &str,
        at: Timestamp,
        error: Option<&str>,
    ) -> Result<bool, StoreError> {
        validate_text("registry url", url)?;
        let done = match error {
            None => run!(&self.writer, |e| sqlx::query(
                "UPDATE plugin_registry SET checked_at = ?, last_error = NULL, \
                 last_error_at = NULL WHERE url = ?"
            )
            .bind(at.as_micros())
            .bind(url)
            .execute(e))?,
            Some(error) => {
                validate_text("registry error", error)?;
                run!(&self.writer, |e| sqlx::query(
                    "UPDATE plugin_registry SET last_error = ?, last_error_at = ? WHERE url = ?"
                )
                .bind(error)
                .bind(at.as_micros())
                .bind(url)
                .execute(e))?
            }
        };
        Ok(done.rows_affected() > 0)
    }
}
