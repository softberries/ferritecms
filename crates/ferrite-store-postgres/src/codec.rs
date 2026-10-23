//! Row decoding. PostgreSQL types (`timestamptz`, `jsonb`, `boolean`) map natively.

use chrono::{DateTime, Utc};
use ferrite_domain::{
    Artifact, ArtifactHash, ContentId, ContentItem, ContentType, Event, InstalledPlugin,
    InstalledTheme, Job, JobId, Media, MenuItem, MenuItemId, Node, NodeId, OptionEntry, Redirect,
    RedirectId, Revision, RevisionId, Secret, Taxonomy, Term, TermId, User, UserId,
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

/// Qualified with the `i` alias so it can be joined against `menus`.
pub(crate) const MENU_ITEM_COLUMNS: &str =
    "i.id, i.parent_id, i.position, i.kind, i.target_id, i.url, i.label";

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

pub(crate) const REDIRECT_COLUMNS: &str =
    "id, source_path, target, code, hits, origin, created_at, last_hit_at";

/// Maps a row selected with [`REDIRECT_COLUMNS`].
pub(crate) fn redirect(row: &PgRow) -> Result<Redirect, StoreError> {
    let id: Uuid = row.try_get("id").map_err(map_db_err)?;
    let code: i32 = row.try_get("code").map_err(map_db_err)?;
    let origin: String = row.try_get("origin").map_err(map_db_err)?;
    let created_at: DateTime<Utc> = row.try_get("created_at").map_err(map_db_err)?;
    let last_hit_at: Option<DateTime<Utc>> = row.try_get("last_hit_at").map_err(map_db_err)?;
    Ok(Redirect {
        id: RedirectId::from_uuid(id),
        source_path: row.try_get("source_path").map_err(map_db_err)?,
        target: row.try_get("target").map_err(map_db_err)?,
        code: u16::try_from(code)
            .map_err(|_| StoreError::Database(format!("corrupt redirect code {code}")))?,
        hits: row.try_get("hits").map_err(map_db_err)?,
        origin: origin.parse()?,
        created_at: created_at.into(),
        last_hit_at: last_hit_at.map(Into::into),
    })
}

pub(crate) const ARTIFACT_COLUMNS: &str = "hash, kind, bytes, mime, created_at";

/// Maps a row selected with [`ARTIFACT_COLUMNS`].
pub(crate) fn artifact(row: &PgRow) -> Result<Artifact, StoreError> {
    let hash: String = row.try_get("hash").map_err(map_db_err)?;
    let kind: String = row.try_get("kind").map_err(map_db_err)?;
    Ok(Artifact {
        hash: ArtifactHash::parse(&hash)?,
        kind: kind.parse()?,
        bytes: row.try_get("bytes").map_err(map_db_err)?,
        mime: row.try_get("mime").map_err(map_db_err)?,
        created_at: row
            .try_get::<DateTime<Utc>, _>("created_at")
            .map_err(map_db_err)?
            .into(),
    })
}

pub(crate) const MEDIA_COLUMNS: &str =
    "item_id, artifact_hash, mime, bytes, width, height, alt, original_name, variants";

/// Maps a row selected with [`MEDIA_COLUMNS`].
pub(crate) fn media(row: &PgRow) -> Result<Media, StoreError> {
    let id: Uuid = row.try_get("item_id").map_err(map_db_err)?;
    let hash: String = row.try_get("artifact_hash").map_err(map_db_err)?;
    let variants: Json<serde_json::Value> = row.try_get("variants").map_err(map_db_err)?;
    Ok(Media {
        item_id: ContentId::from_uuid(id),
        artifact_hash: ArtifactHash::parse(&hash)?,
        mime: row.try_get("mime").map_err(map_db_err)?,
        bytes: row.try_get("bytes").map_err(map_db_err)?,
        width: row.try_get("width").map_err(map_db_err)?,
        height: row.try_get("height").map_err(map_db_err)?,
        alt: row.try_get("alt").map_err(map_db_err)?,
        original_name: row.try_get("original_name").map_err(map_db_err)?,
        variants: variants.0,
    })
}

pub(crate) const PLUGIN_COLUMNS: &str = "name, version, artifact_hash, manifest, granted, \
     enabled, disabled_reason, installed_at, updated_at";

/// Maps a row selected with [`PLUGIN_COLUMNS`].
pub(crate) fn plugin(row: &PgRow) -> Result<InstalledPlugin, StoreError> {
    let hash: String = row.try_get("artifact_hash").map_err(map_db_err)?;
    let manifest: Json<serde_json::Value> = row.try_get("manifest").map_err(map_db_err)?;
    let granted: Json<serde_json::Value> = row.try_get("granted").map_err(map_db_err)?;
    Ok(InstalledPlugin {
        name: row.try_get("name").map_err(map_db_err)?,
        version: row.try_get("version").map_err(map_db_err)?,
        artifact_hash: ArtifactHash::parse(&hash)?,
        manifest: manifest.0,
        granted: granted.0,
        enabled: row.try_get("enabled").map_err(map_db_err)?,
        disabled_reason: row.try_get("disabled_reason").map_err(map_db_err)?,
        installed_at: row
            .try_get::<DateTime<Utc>, _>("installed_at")
            .map_err(map_db_err)?
            .into(),
        updated_at: row
            .try_get::<DateTime<Utc>, _>("updated_at")
            .map_err(map_db_err)?
            .into(),
    })
}

pub(crate) const THEME_COLUMNS: &str =
    "name, version, artifact_hash, manifest, active, installed_at";

/// Maps a row selected with [`THEME_COLUMNS`].
pub(crate) fn theme(row: &PgRow) -> Result<InstalledTheme, StoreError> {
    let hash: String = row.try_get("artifact_hash").map_err(map_db_err)?;
    let manifest: Json<serde_json::Value> = row.try_get("manifest").map_err(map_db_err)?;
    Ok(InstalledTheme {
        name: row.try_get("name").map_err(map_db_err)?,
        version: row.try_get("version").map_err(map_db_err)?,
        artifact_hash: ArtifactHash::parse(&hash)?,
        manifest: manifest.0,
        active: row.try_get("active").map_err(map_db_err)?,
        installed_at: row
            .try_get::<DateTime<Utc>, _>("installed_at")
            .map_err(map_db_err)?
            .into(),
    })
}

/// Maps a `SELECT seq, kind, keys, origin_node, created_at FROM events` row.
pub(crate) fn event(row: &PgRow) -> Result<Event, StoreError> {
    let keys: Json<Vec<String>> = row.try_get("keys").map_err(map_db_err)?;
    Ok(Event {
        seq: row.try_get("seq").map_err(map_db_err)?,
        kind: row.try_get("kind").map_err(map_db_err)?,
        keys: keys.0,
        origin_node: row.try_get("origin_node").map_err(map_db_err)?,
        created_at: row
            .try_get::<DateTime<Utc>, _>("created_at")
            .map_err(map_db_err)?
            .into(),
    })
}

pub(crate) const JOB_COLUMNS: &str = "id, kind, payload, unique_key, run_at, priority, status, \
     attempts, max_attempts, locked_by, locked_until, last_error, created_at, finished_at";

/// `JOB_COLUMNS` qualified with `jobs.` (for `UPDATE ... FROM ... RETURNING`).
pub(crate) const JOB_COLUMNS_QUALIFIED: &str = "jobs.id, jobs.kind, jobs.payload, \
     jobs.unique_key, jobs.run_at, jobs.priority, jobs.status, jobs.attempts, \
     jobs.max_attempts, jobs.locked_by, jobs.locked_until, jobs.last_error, jobs.created_at, \
     jobs.finished_at";

/// Maps a row selected (or returned) with [`JOB_COLUMNS`]. Columns are read by name, so
/// extra columns added by a newer schema are ignored (04 §9).
pub(crate) fn job(row: &PgRow) -> Result<Job, StoreError> {
    let id: Uuid = row.try_get("id").map_err(map_db_err)?;
    let payload: Json<serde_json::Value> = row.try_get("payload").map_err(map_db_err)?;
    let status: String = row.try_get("status").map_err(map_db_err)?;
    let locked_by: Option<String> = row.try_get("locked_by").map_err(map_db_err)?;
    let ts = |name: &str| -> Result<DateTime<Utc>, StoreError> {
        row.try_get::<DateTime<Utc>, _>(name).map_err(map_db_err)
    };
    let ts_opt = |name: &str| -> Result<Option<DateTime<Utc>>, StoreError> {
        row.try_get::<Option<DateTime<Utc>>, _>(name)
            .map_err(map_db_err)
    };
    Ok(Job {
        id: JobId::from_uuid(id),
        kind: row.try_get("kind").map_err(map_db_err)?,
        payload: payload.0,
        unique_key: row.try_get("unique_key").map_err(map_db_err)?,
        run_at: ts("run_at")?.into(),
        priority: row.try_get("priority").map_err(map_db_err)?,
        status: status.parse()?,
        attempts: row.try_get("attempts").map_err(map_db_err)?,
        max_attempts: row.try_get("max_attempts").map_err(map_db_err)?,
        locked_by: locked_by
            .map(|n| n.parse::<NodeId>())
            .transpose()
            .map_err(|e| StoreError::Database(format!("corrupt jobs.locked_by: {e}")))?,
        locked_until: ts_opt("locked_until")?.map(Into::into),
        last_error: row.try_get("last_error").map_err(map_db_err)?,
        created_at: ts("created_at")?.into(),
        finished_at: ts_opt("finished_at")?.map(Into::into),
    })
}

pub(crate) const NODE_COLUMNS: &str =
    "id, name, version, schema_version, started_at, heartbeat_at, meta";

/// Maps a row selected with [`NODE_COLUMNS`].
pub(crate) fn node(row: &PgRow) -> Result<Node, StoreError> {
    let id: Uuid = row.try_get("id").map_err(map_db_err)?;
    let meta: Json<serde_json::Value> = row.try_get("meta").map_err(map_db_err)?;
    let ts = |name: &str| -> Result<DateTime<Utc>, StoreError> {
        row.try_get::<DateTime<Utc>, _>(name).map_err(map_db_err)
    };
    Ok(Node {
        id: NodeId::from_uuid(id),
        name: row.try_get("name").map_err(map_db_err)?,
        version: row.try_get("version").map_err(map_db_err)?,
        schema_version: row.try_get("schema_version").map_err(map_db_err)?,
        started_at: ts("started_at")?.into(),
        heartbeat_at: ts("heartbeat_at")?.into(),
        meta: meta.0,
    })
}
