//! Row decoding. PostgreSQL types (`timestamptz`, `jsonb`, `boolean`) map natively.

use chrono::{DateTime, Utc};
use ferrite_domain::{
    ContentId, ContentItem, ContentType, MenuItem, MenuItemId, OptionEntry, Redirect, RedirectId,
    Revision, RevisionId, Secret, Taxonomy, Term, TermId, User, UserId,
};
use ferrite_store::StoreError;
use sqlx::{Row, postgres::PgRow, types::Json};
use uuid::Uuid;

use crate::exec::map_db_err;

/// Maps a `SELECT key, value, autoload, updated_at FROM options` row.
pub(crate) fn option_entry(row: &PgRow) -> Result<OptionEntry, StoreError> {
    let value: Json<serde_json::Value> = row.try_get("value").map_err(map_db_err)?;
    let updated_at: DateTime<Utc> = row.try_get("updated_at").map_err(map_db_err)?;
    Ok(OptionEntry {
        key: row.try_get("key").map_err(map_db_err)?,
        value: value.0,
        autoload: row.try_get("autoload").map_err(map_db_err)?,
        updated_at: updated_at.into(),
    })
}

pub(crate) const CONTENT_COLUMNS: &str = "id, type, status, slug, parent_id, author_id, title, \
     excerpt, body, body_html, fields, menu_order, published_at, scheduled_at, created_at, \
     updated_at, version";

/// Maps a row selected with [`CONTENT_COLUMNS`].
pub(crate) fn content_item(row: &PgRow) -> Result<ContentItem, StoreError> {
    let id: Uuid = row.try_get("id").map_err(map_db_err)?;
    let parent: Option<Uuid> = row.try_get("parent_id").map_err(map_db_err)?;
    let author: Uuid = row.try_get("author_id").map_err(map_db_err)?;
    let status: String = row.try_get("status").map_err(map_db_err)?;
    let body: Json<serde_json::Value> = row.try_get("body").map_err(map_db_err)?;
    let fields: Json<serde_json::Value> = row.try_get("fields").map_err(map_db_err)?;
    let published_at: Option<DateTime<Utc>> = row.try_get("published_at").map_err(map_db_err)?;
    let scheduled_at: Option<DateTime<Utc>> = row.try_get("scheduled_at").map_err(map_db_err)?;
    let created_at: DateTime<Utc> = row.try_get("created_at").map_err(map_db_err)?;
    let updated_at: DateTime<Utc> = row.try_get("updated_at").map_err(map_db_err)?;
    Ok(ContentItem {
        id: ContentId::from_uuid(id),
        kind: row.try_get("type").map_err(map_db_err)?,
        status: status.parse()?,
        slug: row.try_get("slug").map_err(map_db_err)?,
        parent_id: parent.map(ContentId::from_uuid),
        author_id: UserId::from_uuid(author),
        title: row.try_get("title").map_err(map_db_err)?,
        excerpt: row.try_get("excerpt").map_err(map_db_err)?,
        body: body.0,
        body_html: row.try_get("body_html").map_err(map_db_err)?,
        fields: fields.0,
        menu_order: row.try_get("menu_order").map_err(map_db_err)?,
        published_at: published_at.map(Into::into),
        scheduled_at: scheduled_at.map(Into::into),
        created_at: created_at.into(),
        updated_at: updated_at.into(),
        version: row.try_get("version").map_err(map_db_err)?,
    })
}

pub(crate) const TYPE_COLUMNS: &str =
    "name, label, hierarchical, has_archive, fields, built_in, created_at";

/// Maps a row selected with [`TYPE_COLUMNS`].
pub(crate) fn content_type(row: &PgRow) -> Result<ContentType, StoreError> {
    let fields: Json<serde_json::Value> = row.try_get("fields").map_err(map_db_err)?;
    let created_at: DateTime<Utc> = row.try_get("created_at").map_err(map_db_err)?;
    Ok(ContentType {
        name: row.try_get("name").map_err(map_db_err)?,
        label: row.try_get("label").map_err(map_db_err)?,
        hierarchical: row.try_get("hierarchical").map_err(map_db_err)?,
        has_archive: row.try_get("has_archive").map_err(map_db_err)?,
        fields: fields.0,
        built_in: row.try_get("built_in").map_err(map_db_err)?,
        created_at: created_at.into(),
    })
}

pub(crate) const USER_COLUMNS: &str = "id, login, email, display_name, password_hash, role, \
     status, must_reset, totp_secret, created_at, last_login_at";

/// Maps a row selected with [`USER_COLUMNS`].
pub(crate) fn user(row: &PgRow) -> Result<User, StoreError> {
    let id: Uuid = row.try_get("id").map_err(map_db_err)?;
    let password_hash: Option<String> = row.try_get("password_hash").map_err(map_db_err)?;
    let totp_secret: Option<String> = row.try_get("totp_secret").map_err(map_db_err)?;
    let role: String = row.try_get("role").map_err(map_db_err)?;
    let status: String = row.try_get("status").map_err(map_db_err)?;
    let created_at: DateTime<Utc> = row.try_get("created_at").map_err(map_db_err)?;
    let last_login_at: Option<DateTime<Utc>> = row.try_get("last_login_at").map_err(map_db_err)?;
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
        created_at: created_at.into(),
        last_login_at: last_login_at.map(Into::into),
    })
}

pub(crate) const REVISION_COLUMNS: &str =
    "id, item_id, author_id, created_at, title, excerpt, body, fields, meta, note";

/// Maps a row selected with [`REVISION_COLUMNS`].
pub(crate) fn revision(row: &PgRow) -> Result<Revision, StoreError> {
    let id: Uuid = row.try_get("id").map_err(map_db_err)?;
    let item: Uuid = row.try_get("item_id").map_err(map_db_err)?;
    let author: Uuid = row.try_get("author_id").map_err(map_db_err)?;
    let created_at: DateTime<Utc> = row.try_get("created_at").map_err(map_db_err)?;
    let body: Json<serde_json::Value> = row.try_get("body").map_err(map_db_err)?;
    let fields: Json<serde_json::Value> = row.try_get("fields").map_err(map_db_err)?;
    let meta: Json<serde_json::Value> = row.try_get("meta").map_err(map_db_err)?;
    Ok(Revision {
        id: RevisionId::from_uuid(id),
        item_id: ContentId::from_uuid(item),
        author_id: UserId::from_uuid(author),
        created_at: created_at.into(),
        title: row.try_get("title").map_err(map_db_err)?,
        excerpt: row.try_get("excerpt").map_err(map_db_err)?,
        body: body.0,
        fields: fields.0,
        meta: meta.0,
        note: row.try_get("note").map_err(map_db_err)?,
    })
}

pub(crate) const TAXONOMY_COLUMNS: &str = "name, label, hierarchical, content_types, built_in";

/// Maps a row selected with [`TAXONOMY_COLUMNS`].
pub(crate) fn taxonomy(row: &PgRow) -> Result<Taxonomy, StoreError> {
    let types: Json<Vec<String>> = row.try_get("content_types").map_err(map_db_err)?;
    Ok(Taxonomy {
        name: row.try_get("name").map_err(map_db_err)?,
        label: row.try_get("label").map_err(map_db_err)?,
        hierarchical: row.try_get("hierarchical").map_err(map_db_err)?,
        content_types: types.0,
        built_in: row.try_get("built_in").map_err(map_db_err)?,
    })
}

/// Term columns qualified with the `t` alias so joins stay unambiguous.
pub(crate) const TERM_COLUMNS: &str =
    "t.id, t.taxonomy, t.slug, t.name, t.description, t.parent_id";

/// Maps a row selected with [`TERM_COLUMNS`].
pub(crate) fn term(row: &PgRow) -> Result<Term, StoreError> {
    let id: Uuid = row.try_get("id").map_err(map_db_err)?;
    let parent: Option<Uuid> = row.try_get("parent_id").map_err(map_db_err)?;
    Ok(Term {
        id: TermId::from_uuid(id),
        taxonomy: row.try_get("taxonomy").map_err(map_db_err)?,
        slug: row.try_get("slug").map_err(map_db_err)?,
        name: row.try_get("name").map_err(map_db_err)?,
        description: row.try_get("description").map_err(map_db_err)?,
        parent_id: parent.map(TermId::from_uuid),
    })
}

pub(crate) const MENU_ITEM_COLUMNS: &str = "id, parent_id, position, kind, target_id, url, label";

/// Maps a row selected with [`MENU_ITEM_COLUMNS`].
pub(crate) fn menu_item(row: &PgRow) -> Result<MenuItem, StoreError> {
    let id: Uuid = row.try_get("id").map_err(map_db_err)?;
    let parent: Option<Uuid> = row.try_get("parent_id").map_err(map_db_err)?;
    let kind: String = row.try_get("kind").map_err(map_db_err)?;
    Ok(MenuItem {
        id: MenuItemId::from_uuid(id),
        parent_id: parent.map(MenuItemId::from_uuid),
        position: row.try_get("position").map_err(map_db_err)?,
        kind: kind.parse()?,
        target_id: row.try_get("target_id").map_err(map_db_err)?,
        url: row.try_get("url").map_err(map_db_err)?,
        label: row.try_get("label").map_err(map_db_err)?,
    })
}

pub(crate) const REDIRECT_COLUMNS: &str = "id, source_path, target, code, hits, origin, created_at";

/// Maps a row selected with [`REDIRECT_COLUMNS`].
pub(crate) fn redirect(row: &PgRow) -> Result<Redirect, StoreError> {
    let id: Uuid = row.try_get("id").map_err(map_db_err)?;
    let code: i32 = row.try_get("code").map_err(map_db_err)?;
    let origin: String = row.try_get("origin").map_err(map_db_err)?;
    let created_at: DateTime<Utc> = row.try_get("created_at").map_err(map_db_err)?;
    Ok(Redirect {
        id: RedirectId::from_uuid(id),
        source_path: row.try_get("source_path").map_err(map_db_err)?,
        target: row.try_get("target").map_err(map_db_err)?,
        code: u16::try_from(code)
            .map_err(|_| StoreError::Database(format!("corrupt redirect code {code}")))?,
        hits: row.try_get("hits").map_err(map_db_err)?,
        origin: origin.parse()?,
        created_at: created_at.into(),
    })
}
