//! SQLite physical encodings (02 §5): timestamp = unix microseconds `INTEGER`,
//! JSON = `TEXT`. Booleans bind and decode natively as 0/1.

use ferrite_domain::{
    ContentId, ContentItem, ContentType, OptionEntry, Secret, Timestamp, User, UserId,
};
use ferrite_store::StoreError;
use sqlx::{Row, sqlite::SqliteRow};
use uuid::Uuid;

use crate::exec::map_db_err;

pub(crate) fn json_to_db(value: &serde_json::Value) -> Result<String, StoreError> {
    serde_json::to_string(value).map_err(|e| StoreError::Invalid(e.to_string()))
}

pub(crate) fn json_from_db(text: &str) -> Result<serde_json::Value, StoreError> {
    serde_json::from_str(text).map_err(|e| StoreError::Database(format!("corrupt JSON: {e}")))
}

/// Maps a `SELECT key, value, autoload, updated_at FROM options` row.
pub(crate) fn option_entry(row: &SqliteRow) -> Result<OptionEntry, StoreError> {
    let value: String = row.try_get("value").map_err(map_db_err)?;
    let micros: i64 = row.try_get("updated_at").map_err(map_db_err)?;
    Ok(OptionEntry {
        key: row.try_get("key").map_err(map_db_err)?,
        value: json_from_db(&value)?,
        autoload: row.try_get("autoload").map_err(map_db_err)?,
        updated_at: ts(micros)?,
    })
}

fn ts(micros: i64) -> Result<Timestamp, StoreError> {
    Ok(Timestamp::from_micros(micros)?)
}

fn ts_opt(micros: Option<i64>) -> Result<Option<Timestamp>, StoreError> {
    micros.map(ts).transpose()
}

pub(crate) const CONTENT_COLUMNS: &str = "id, type, status, slug, parent_id, author_id, title, \
     excerpt, body, body_html, fields, menu_order, published_at, scheduled_at, created_at, \
     updated_at, version";

/// Maps a row selected with [`CONTENT_COLUMNS`].
pub(crate) fn content_item(row: &SqliteRow) -> Result<ContentItem, StoreError> {
    let id: Uuid = row.try_get("id").map_err(map_db_err)?;
    let parent: Option<Uuid> = row.try_get("parent_id").map_err(map_db_err)?;
    let author: Uuid = row.try_get("author_id").map_err(map_db_err)?;
    let status: String = row.try_get("status").map_err(map_db_err)?;
    let body: String = row.try_get("body").map_err(map_db_err)?;
    let fields: String = row.try_get("fields").map_err(map_db_err)?;
    Ok(ContentItem {
        id: ContentId::from_uuid(id),
        kind: row.try_get("type").map_err(map_db_err)?,
        status: status.parse()?,
        slug: row.try_get("slug").map_err(map_db_err)?,
        parent_id: parent.map(ContentId::from_uuid),
        author_id: UserId::from_uuid(author),
        title: row.try_get("title").map_err(map_db_err)?,
        excerpt: row.try_get("excerpt").map_err(map_db_err)?,
        body: json_from_db(&body)?,
        body_html: row.try_get("body_html").map_err(map_db_err)?,
        fields: json_from_db(&fields)?,
        menu_order: row.try_get("menu_order").map_err(map_db_err)?,
        published_at: ts_opt(row.try_get("published_at").map_err(map_db_err)?)?,
        scheduled_at: ts_opt(row.try_get("scheduled_at").map_err(map_db_err)?)?,
        created_at: ts(row.try_get("created_at").map_err(map_db_err)?)?,
        updated_at: ts(row.try_get("updated_at").map_err(map_db_err)?)?,
        version: row.try_get("version").map_err(map_db_err)?,
    })
}

pub(crate) const TYPE_COLUMNS: &str =
    "name, label, hierarchical, has_archive, fields, built_in, created_at";

/// Maps a row selected with [`TYPE_COLUMNS`].
pub(crate) fn content_type(row: &SqliteRow) -> Result<ContentType, StoreError> {
    let fields: String = row.try_get("fields").map_err(map_db_err)?;
    Ok(ContentType {
        name: row.try_get("name").map_err(map_db_err)?,
        label: row.try_get("label").map_err(map_db_err)?,
        hierarchical: row.try_get("hierarchical").map_err(map_db_err)?,
        has_archive: row.try_get("has_archive").map_err(map_db_err)?,
        fields: json_from_db(&fields)?,
        built_in: row.try_get("built_in").map_err(map_db_err)?,
        created_at: ts(row.try_get("created_at").map_err(map_db_err)?)?,
    })
}

pub(crate) const USER_COLUMNS: &str = "id, login, email, display_name, password_hash, role, \
     status, must_reset, totp_secret, created_at, last_login_at";

/// Maps a row selected with [`USER_COLUMNS`].
pub(crate) fn user(row: &SqliteRow) -> Result<User, StoreError> {
    let id: Uuid = row.try_get("id").map_err(map_db_err)?;
    let password_hash: Option<String> = row.try_get("password_hash").map_err(map_db_err)?;
    let totp_secret: Option<String> = row.try_get("totp_secret").map_err(map_db_err)?;
    let role: String = row.try_get("role").map_err(map_db_err)?;
    let status: String = row.try_get("status").map_err(map_db_err)?;
    Ok(User {
        id: UserId::from_uuid(id),
        login: row.try_get("login").map_err(map_db_err)?,
        email: row.try_get("email").map_err(map_db_err)?,
        display_name: row.try_get("display_name").map_err(map_db_err)?,
        password_hash: password_hash.map(Secret::new),
        role: role.parse()?,
        status: status.parse()?,
        must_reset: row.try_get("must_reset").map_err(map_db_err)?,
        totp_secret: totp_secret.map(Secret::new),
        created_at: ts(row.try_get("created_at").map_err(map_db_err)?)?,
        last_login_at: ts_opt(row.try_get("last_login_at").map_err(map_db_err)?)?,
    })
}
