//! SQLite physical encodings (02 §5): timestamp = unix microseconds `INTEGER`,
//! JSON = `TEXT`. Booleans bind and decode natively as 0/1.

use ferrite_domain::{
    Artifact, ArtifactHash, ContentId, ContentItem, ContentType, Event, InstalledPlugin,
    InstalledTheme, Media, MenuItem, MenuItemId, OptionEntry, Redirect, RedirectId, Revision,
    RevisionId, Secret, Taxonomy, Term, TermId, Timestamp, User, UserId,
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

pub(crate) const REVISION_COLUMNS: &str =
    "id, item_id, author_id, created_at, title, excerpt, body, fields, meta, note";

/// Maps a row selected with [`REVISION_COLUMNS`].
pub(crate) fn revision(row: &SqliteRow) -> Result<Revision, StoreError> {
    let id: Uuid = row.try_get("id").map_err(map_db_err)?;
    let item: Uuid = row.try_get("item_id").map_err(map_db_err)?;
    let author: Uuid = row.try_get("author_id").map_err(map_db_err)?;
    let body: String = row.try_get("body").map_err(map_db_err)?;
    let fields: String = row.try_get("fields").map_err(map_db_err)?;
    let meta: String = row.try_get("meta").map_err(map_db_err)?;
    Ok(Revision {
        id: RevisionId::from_uuid(id),
        item_id: ContentId::from_uuid(item),
        author_id: UserId::from_uuid(author),
        created_at: ts(row.try_get("created_at").map_err(map_db_err)?)?,
        title: row.try_get("title").map_err(map_db_err)?,
        excerpt: row.try_get("excerpt").map_err(map_db_err)?,
        body: json_from_db(&body)?,
        fields: json_from_db(&fields)?,
        meta: json_from_db(&meta)?,
        note: row.try_get("note").map_err(map_db_err)?,
    })
}

pub(crate) const TAXONOMY_COLUMNS: &str = "name, label, hierarchical, content_types, built_in";

/// Maps a row selected with [`TAXONOMY_COLUMNS`].
pub(crate) fn taxonomy(row: &SqliteRow) -> Result<Taxonomy, StoreError> {
    let types: String = row.try_get("content_types").map_err(map_db_err)?;
    Ok(Taxonomy {
        name: row.try_get("name").map_err(map_db_err)?,
        label: row.try_get("label").map_err(map_db_err)?,
        hierarchical: row.try_get("hierarchical").map_err(map_db_err)?,
        content_types: serde_json::from_str(&types)
            .map_err(|e| StoreError::Database(format!("corrupt JSON: {e}")))?,
        built_in: row.try_get("built_in").map_err(map_db_err)?,
    })
}

/// Term columns qualified with the `t` alias so joins stay unambiguous.
pub(crate) const TERM_COLUMNS: &str =
    "t.id, t.taxonomy, t.slug, t.name, t.description, t.parent_id";

/// Maps a row selected with [`TERM_COLUMNS`].
pub(crate) fn term(row: &SqliteRow) -> Result<Term, StoreError> {
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

/// Qualified with the `i` alias so it can be joined against `menus`.
pub(crate) const MENU_ITEM_COLUMNS: &str =
    "i.id, i.parent_id, i.position, i.kind, i.target_id, i.url, i.label";

/// Maps a row selected with [`MENU_ITEM_COLUMNS`].
pub(crate) fn menu_item(row: &SqliteRow) -> Result<MenuItem, StoreError> {
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
pub(crate) fn redirect(row: &SqliteRow) -> Result<Redirect, StoreError> {
    let id: Uuid = row.try_get("id").map_err(map_db_err)?;
    let code: i64 = row.try_get("code").map_err(map_db_err)?;
    let origin: String = row.try_get("origin").map_err(map_db_err)?;
    Ok(Redirect {
        id: RedirectId::from_uuid(id),
        source_path: row.try_get("source_path").map_err(map_db_err)?,
        target: row.try_get("target").map_err(map_db_err)?,
        code: u16::try_from(code)
            .map_err(|_| StoreError::Database(format!("corrupt redirect code {code}")))?,
        hits: row.try_get("hits").map_err(map_db_err)?,
        origin: origin.parse()?,
        created_at: ts(row.try_get("created_at").map_err(map_db_err)?)?,
    })
}

pub(crate) const ARTIFACT_COLUMNS: &str = "hash, kind, bytes, mime, created_at";

/// Maps a row selected with [`ARTIFACT_COLUMNS`].
pub(crate) fn artifact(row: &SqliteRow) -> Result<Artifact, StoreError> {
    let hash: String = row.try_get("hash").map_err(map_db_err)?;
    let kind: String = row.try_get("kind").map_err(map_db_err)?;
    Ok(Artifact {
        hash: ArtifactHash::parse(&hash)?,
        kind: kind.parse()?,
        bytes: row.try_get("bytes").map_err(map_db_err)?,
        mime: row.try_get("mime").map_err(map_db_err)?,
        created_at: ts(row.try_get("created_at").map_err(map_db_err)?)?,
    })
}

pub(crate) const MEDIA_COLUMNS: &str =
    "item_id, artifact_hash, mime, bytes, width, height, alt, original_name, variants";

/// Maps a row selected with [`MEDIA_COLUMNS`].
pub(crate) fn media(row: &SqliteRow) -> Result<Media, StoreError> {
    let id: Uuid = row.try_get("item_id").map_err(map_db_err)?;
    let hash: String = row.try_get("artifact_hash").map_err(map_db_err)?;
    let variants: String = row.try_get("variants").map_err(map_db_err)?;
    Ok(Media {
        item_id: ContentId::from_uuid(id),
        artifact_hash: ArtifactHash::parse(&hash)?,
        mime: row.try_get("mime").map_err(map_db_err)?,
        bytes: row.try_get("bytes").map_err(map_db_err)?,
        width: row.try_get("width").map_err(map_db_err)?,
        height: row.try_get("height").map_err(map_db_err)?,
        alt: row.try_get("alt").map_err(map_db_err)?,
        original_name: row.try_get("original_name").map_err(map_db_err)?,
        variants: json_from_db(&variants)?,
    })
}

pub(crate) const PLUGIN_COLUMNS: &str = "name, version, artifact_hash, manifest, granted, \
     enabled, disabled_reason, installed_at, updated_at";

/// Maps a row selected with [`PLUGIN_COLUMNS`].
pub(crate) fn plugin(row: &SqliteRow) -> Result<InstalledPlugin, StoreError> {
    let hash: String = row.try_get("artifact_hash").map_err(map_db_err)?;
    let manifest: String = row.try_get("manifest").map_err(map_db_err)?;
    let granted: String = row.try_get("granted").map_err(map_db_err)?;
    Ok(InstalledPlugin {
        name: row.try_get("name").map_err(map_db_err)?,
        version: row.try_get("version").map_err(map_db_err)?,
        artifact_hash: ArtifactHash::parse(&hash)?,
        manifest: json_from_db(&manifest)?,
        granted: json_from_db(&granted)?,
        enabled: row.try_get("enabled").map_err(map_db_err)?,
        disabled_reason: row.try_get("disabled_reason").map_err(map_db_err)?,
        installed_at: ts(row.try_get("installed_at").map_err(map_db_err)?)?,
        updated_at: ts(row.try_get("updated_at").map_err(map_db_err)?)?,
    })
}

pub(crate) const THEME_COLUMNS: &str =
    "name, version, artifact_hash, manifest, active, installed_at";

/// Maps a row selected with [`THEME_COLUMNS`].
pub(crate) fn theme(row: &SqliteRow) -> Result<InstalledTheme, StoreError> {
    let hash: String = row.try_get("artifact_hash").map_err(map_db_err)?;
    let manifest: String = row.try_get("manifest").map_err(map_db_err)?;
    Ok(InstalledTheme {
        name: row.try_get("name").map_err(map_db_err)?,
        version: row.try_get("version").map_err(map_db_err)?,
        artifact_hash: ArtifactHash::parse(&hash)?,
        manifest: json_from_db(&manifest)?,
        active: row.try_get("active").map_err(map_db_err)?,
        installed_at: ts(row.try_get("installed_at").map_err(map_db_err)?)?,
    })
}

/// Maps a `SELECT seq, kind, keys, origin_node, created_at FROM events` row.
pub(crate) fn event(row: &SqliteRow) -> Result<Event, StoreError> {
    let keys: String = row.try_get("keys").map_err(map_db_err)?;
    Ok(Event {
        seq: row.try_get("seq").map_err(map_db_err)?,
        kind: row.try_get("kind").map_err(map_db_err)?,
        keys: serde_json::from_str(&keys)
            .map_err(|e| StoreError::Database(format!("corrupt JSON: {e}")))?,
        origin_node: row.try_get("origin_node").map_err(map_db_err)?,
        created_at: ts(row.try_get("created_at").map_err(map_db_err)?)?,
    })
}
