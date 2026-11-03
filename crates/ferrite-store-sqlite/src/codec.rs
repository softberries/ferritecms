//! SQLite physical encodings (02 §5): timestamp = unix microseconds `INTEGER`,
//! JSON = `TEXT`. Booleans bind and decode natively as 0/1.

use ferrite_domain::{
    ApiToken, ApiTokenId, Artifact, ArtifactHash, AuditEntry, AuditId, ClientKind, Comment,
    CommentId, ContentId, ContentItem, ContentType, Event, InstalledPlugin, InstalledTheme, Job,
    JobId, Media, MenuItem, MenuItemId, Node, NodeId, OAuthClient, OAuthClientId, OAuthCode,
    OAuthGrant, OAuthGrantId, OAuthRequest, OAuthRequestId, OptionEntry, Redirect, RedirectId,
    Revision, RevisionId, Secret, Session, Taxonomy, Term, TermId, Timestamp, User, UserId,
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
     updated_at, version, locale, translation_group";

/// Maps a `group_members` row (`id, grp, type, locale, status, slug, parent_id,
/// published_at, title`).
pub(crate) fn translation_ref(
    row: &SqliteRow,
) -> Result<ferrite_store::TranslationRef, StoreError> {
    let id: Uuid = row.try_get("id").map_err(map_db_err)?;
    let group: Uuid = row.try_get("grp").map_err(map_db_err)?;
    let parent: Option<Uuid> = row.try_get("parent_id").map_err(map_db_err)?;
    let status: String = row.try_get("status").map_err(map_db_err)?;
    Ok(ferrite_store::TranslationRef {
        id: ContentId::from_uuid(id),
        group: ContentId::from_uuid(group),
        kind: row.try_get("type").map_err(map_db_err)?,
        locale: row.try_get("locale").map_err(map_db_err)?,
        status: status.parse()?,
        slug: row.try_get("slug").map_err(map_db_err)?,
        parent_id: parent.map(ContentId::from_uuid),
        published_at: ts_opt(row.try_get("published_at").map_err(map_db_err)?)?,
        title: row.try_get("title").map_err(map_db_err)?,
        author_id: ferrite_domain::UserId::from_uuid(
            row.try_get::<Uuid, _>("author_id").map_err(map_db_err)?,
        ),
    })
}

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
        locale: row.try_get("locale").map_err(map_db_err)?,
        translation_group: ContentId::from_uuid(
            row.try_get::<Option<Uuid>, _>("translation_group")
                .map_err(map_db_err)?
                .unwrap_or(id),
        ),
    })
}

pub(crate) const TYPE_COLUMNS: &str =
    "name, label, hierarchical, has_archive, fields, built_in, created_at, quality_gate";

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
        quality_gate: {
            let g: String = row.try_get("quality_gate").map_err(map_db_err)?;
            g.parse()?
        },
    })
}

pub(crate) const USER_COLUMNS: &str = "id, login, email, display_name, password_hash, role, \
     status, must_reset, totp_secret, totp_last_step, created_at, last_login_at";

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
        totp_last_step: row.try_get("totp_last_step").map_err(map_db_err)?,
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
    "t.id, t.taxonomy, t.slug, t.name, t.description, t.parent_id, t.locale, t.translation_group";

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
        locale: row.try_get("locale").map_err(map_db_err)?,
        translation_group: TermId::from_uuid(
            row.try_get::<Option<Uuid>, _>("translation_group")
                .map_err(map_db_err)?
                .unwrap_or(id),
        ),
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

pub(crate) const REDIRECT_COLUMNS: &str =
    "id, source_path, target, code, hits, origin, created_at, last_hit_at";

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
        last_hit_at: ts_opt(row.try_get("last_hit_at").map_err(map_db_err)?)?,
    })
}

pub(crate) const ARTIFACT_COLUMNS: &str = "hash, kind, bytes, mime, created_at, orphaned_at";

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
        orphaned_at: ts_opt(row.try_get("orphaned_at").map_err(map_db_err)?)?,
    })
}

pub(crate) const MEDIA_COLUMNS: &str =
    "item_id, artifact_hash, mime, bytes, width, height, alt, original_name, variants, sanitized";

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
        sanitized: row.try_get("sanitized").map_err(map_db_err)?,
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

pub(crate) const JOB_COLUMNS: &str = "id, kind, payload, unique_key, run_at, priority, status, \
     attempts, max_attempts, locked_by, locked_until, last_error, created_at, finished_at";

/// Maps a row selected (or returned) with [`JOB_COLUMNS`]. Columns are read by name, so
/// extra columns added by a newer schema are ignored (04 §9).
pub(crate) fn job(row: &SqliteRow) -> Result<Job, StoreError> {
    let id: Uuid = row.try_get("id").map_err(map_db_err)?;
    let payload: String = row.try_get("payload").map_err(map_db_err)?;
    let status: String = row.try_get("status").map_err(map_db_err)?;
    let locked_by: Option<String> = row.try_get("locked_by").map_err(map_db_err)?;
    Ok(Job {
        id: JobId::from_uuid(id),
        kind: row.try_get("kind").map_err(map_db_err)?,
        payload: json_from_db(&payload)?,
        unique_key: row.try_get("unique_key").map_err(map_db_err)?,
        run_at: ts(row.try_get("run_at").map_err(map_db_err)?)?,
        priority: row.try_get("priority").map_err(map_db_err)?,
        status: status.parse()?,
        attempts: row.try_get("attempts").map_err(map_db_err)?,
        max_attempts: row.try_get("max_attempts").map_err(map_db_err)?,
        locked_by: locked_by
            .map(|n| n.parse::<NodeId>())
            .transpose()
            .map_err(|e| StoreError::Database(format!("corrupt jobs.locked_by: {e}")))?,
        locked_until: ts_opt(row.try_get("locked_until").map_err(map_db_err)?)?,
        last_error: row.try_get("last_error").map_err(map_db_err)?,
        created_at: ts(row.try_get("created_at").map_err(map_db_err)?)?,
        finished_at: ts_opt(row.try_get("finished_at").map_err(map_db_err)?)?,
    })
}

pub(crate) const NODE_COLUMNS: &str =
    "id, name, version, schema_version, started_at, heartbeat_at, meta";

/// Maps a row selected with [`NODE_COLUMNS`].
pub(crate) fn node(row: &SqliteRow) -> Result<Node, StoreError> {
    let id: Uuid = row.try_get("id").map_err(map_db_err)?;
    let meta: String = row.try_get("meta").map_err(map_db_err)?;
    Ok(Node {
        id: NodeId::from_uuid(id),
        name: row.try_get("name").map_err(map_db_err)?,
        version: row.try_get("version").map_err(map_db_err)?,
        schema_version: row.try_get("schema_version").map_err(map_db_err)?,
        started_at: ts(row.try_get("started_at").map_err(map_db_err)?)?,
        heartbeat_at: ts(row.try_get("heartbeat_at").map_err(map_db_err)?)?,
        meta: json_from_db(&meta)?,
    })
}

pub(crate) const SESSION_COLUMNS: &str = "id_hash, user_id, csrf_secret, created_at, \
     last_seen_at, expires_at, ip_hash, user_agent";

/// Maps a row selected with [`SESSION_COLUMNS`].
pub(crate) fn session(row: &SqliteRow) -> Result<Session, StoreError> {
    let user: Uuid = row.try_get("user_id").map_err(map_db_err)?;
    let csrf: String = row.try_get("csrf_secret").map_err(map_db_err)?;
    Ok(Session {
        id_hash: row.try_get("id_hash").map_err(map_db_err)?,
        user_id: UserId::from_uuid(user),
        csrf_secret: Secret::new(csrf),
        created_at: ts(row.try_get("created_at").map_err(map_db_err)?)?,
        last_seen_at: ts(row.try_get("last_seen_at").map_err(map_db_err)?)?,
        expires_at: ts(row.try_get("expires_at").map_err(map_db_err)?)?,
        ip_hash: row.try_get("ip_hash").map_err(map_db_err)?,
        user_agent: row.try_get("user_agent").map_err(map_db_err)?,
    })
}

pub(crate) const API_TOKEN_COLUMNS: &str =
    "id, user_id, name, token_hash, scopes, created_at, last_used_at, expires_at, oauth_grant_id";

/// Maps a row selected with [`API_TOKEN_COLUMNS`].
pub(crate) fn api_token(row: &SqliteRow) -> Result<ApiToken, StoreError> {
    let id: Uuid = row.try_get("id").map_err(map_db_err)?;
    let user: Uuid = row.try_get("user_id").map_err(map_db_err)?;
    let scopes: String = row.try_get("scopes").map_err(map_db_err)?;
    let grant: Option<Uuid> = row.try_get("oauth_grant_id").map_err(map_db_err)?;
    Ok(ApiToken {
        id: ApiTokenId::from_uuid(id),
        user_id: UserId::from_uuid(user),
        name: row.try_get("name").map_err(map_db_err)?,
        token_hash: row.try_get("token_hash").map_err(map_db_err)?,
        scopes: serde_json::from_str(&scopes)
            .map_err(|e| StoreError::Database(format!("corrupt token scopes: {e}")))?,
        created_at: ts(row.try_get("created_at").map_err(map_db_err)?)?,
        last_used_at: ts_opt(row.try_get("last_used_at").map_err(map_db_err)?)?,
        expires_at: ts_opt(row.try_get("expires_at").map_err(map_db_err)?)?,
        oauth_grant_id: grant.map(OAuthGrantId::from_uuid),
    })
}

fn string_list(row: &SqliteRow, column: &str) -> Result<Vec<String>, StoreError> {
    let text: String = row.try_get(column).map_err(map_db_err)?;
    serde_json::from_str(&text)
        .map_err(|e| StoreError::Database(format!("corrupt oauth {column}: {e}")))
}

pub(crate) const OAUTH_CLIENT_COLUMNS: &str =
    "id, kind, name, redirect_uris, client_uri, logo_uri, created_at, fetched_at, last_used_at";

/// Maps a row selected with [`OAUTH_CLIENT_COLUMNS`].
pub(crate) fn oauth_client(row: &SqliteRow) -> Result<OAuthClient, StoreError> {
    let id: String = row.try_get("id").map_err(map_db_err)?;
    let kind: String = row.try_get("kind").map_err(map_db_err)?;
    Ok(OAuthClient {
        id: OAuthClientId::new(id),
        kind: ClientKind::parse(&kind)
            .ok_or_else(|| StoreError::Database(format!("corrupt oauth client kind {kind}")))?,
        name: row.try_get("name").map_err(map_db_err)?,
        redirect_uris: string_list(row, "redirect_uris")?,
        client_uri: row.try_get("client_uri").map_err(map_db_err)?,
        logo_uri: row.try_get("logo_uri").map_err(map_db_err)?,
        created_at: ts(row.try_get("created_at").map_err(map_db_err)?)?,
        fetched_at: ts_opt(row.try_get("fetched_at").map_err(map_db_err)?)?,
        last_used_at: ts_opt(row.try_get("last_used_at").map_err(map_db_err)?)?,
    })
}

pub(crate) const OAUTH_REQUEST_COLUMNS: &str =
    "id, client_id, redirect_uri, scopes, state, code_challenge, resource, created_at, expires_at";

/// Maps a row selected with [`OAUTH_REQUEST_COLUMNS`].
pub(crate) fn oauth_request(row: &SqliteRow) -> Result<OAuthRequest, StoreError> {
    let id: Uuid = row.try_get("id").map_err(map_db_err)?;
    let client: String = row.try_get("client_id").map_err(map_db_err)?;
    Ok(OAuthRequest {
        id: OAuthRequestId::from_uuid(id),
        client_id: OAuthClientId::new(client),
        redirect_uri: row.try_get("redirect_uri").map_err(map_db_err)?,
        scopes: string_list(row, "scopes")?,
        state: row.try_get("state").map_err(map_db_err)?,
        code_challenge: row.try_get("code_challenge").map_err(map_db_err)?,
        resource: row.try_get("resource").map_err(map_db_err)?,
        created_at: ts(row.try_get("created_at").map_err(map_db_err)?)?,
        expires_at: ts(row.try_get("expires_at").map_err(map_db_err)?)?,
    })
}

pub(crate) const OAUTH_CODE_COLUMNS: &str = "code_hash, client_id, user_id, redirect_uri, scopes, \
     code_challenge, resource, created_at, expires_at, grant_id";

/// Maps a row selected with [`OAUTH_CODE_COLUMNS`].
pub(crate) fn oauth_code(row: &SqliteRow) -> Result<OAuthCode, StoreError> {
    let client: String = row.try_get("client_id").map_err(map_db_err)?;
    let user: Uuid = row.try_get("user_id").map_err(map_db_err)?;
    let grant: Option<Uuid> = row.try_get("grant_id").map_err(map_db_err)?;
    Ok(OAuthCode {
        code_hash: row.try_get("code_hash").map_err(map_db_err)?,
        client_id: OAuthClientId::new(client),
        user_id: UserId::from_uuid(user),
        redirect_uri: row.try_get("redirect_uri").map_err(map_db_err)?,
        scopes: string_list(row, "scopes")?,
        code_challenge: row.try_get("code_challenge").map_err(map_db_err)?,
        resource: row.try_get("resource").map_err(map_db_err)?,
        created_at: ts(row.try_get("created_at").map_err(map_db_err)?)?,
        expires_at: ts(row.try_get("expires_at").map_err(map_db_err)?)?,
        grant_id: grant.map(OAuthGrantId::from_uuid),
    })
}

pub(crate) const OAUTH_GRANT_COLUMNS: &str =
    "id, client_id, user_id, scopes, created_at, last_used_at, expires_at";

/// Maps a row selected with [`OAUTH_GRANT_COLUMNS`]; the refresh hashes are not selected.
pub(crate) fn oauth_grant(row: &SqliteRow) -> Result<OAuthGrant, StoreError> {
    let id: Uuid = row.try_get("id").map_err(map_db_err)?;
    let client: String = row.try_get("client_id").map_err(map_db_err)?;
    let user: Uuid = row.try_get("user_id").map_err(map_db_err)?;
    Ok(OAuthGrant {
        id: OAuthGrantId::from_uuid(id),
        client_id: OAuthClientId::new(client),
        user_id: UserId::from_uuid(user),
        scopes: string_list(row, "scopes")?,
        created_at: ts(row.try_get("created_at").map_err(map_db_err)?)?,
        last_used_at: ts(row.try_get("last_used_at").map_err(map_db_err)?)?,
        expires_at: ts(row.try_get("expires_at").map_err(map_db_err)?)?,
    })
}

pub(crate) const AUDIT_COLUMNS: &str =
    "id, at, actor_id, action, target_kind, target_id, details, node";

/// Maps a row selected with [`AUDIT_COLUMNS`].
pub(crate) fn audit_entry(row: &SqliteRow) -> Result<AuditEntry, StoreError> {
    let id: Uuid = row.try_get("id").map_err(map_db_err)?;
    let actor: Option<Uuid> = row.try_get("actor_id").map_err(map_db_err)?;
    let details: String = row.try_get("details").map_err(map_db_err)?;
    let node: Option<String> = row.try_get("node").map_err(map_db_err)?;
    Ok(AuditEntry {
        id: AuditId::from_uuid(id),
        at: ts(row.try_get("at").map_err(map_db_err)?)?,
        actor_id: actor.map(UserId::from_uuid),
        action: row.try_get("action").map_err(map_db_err)?,
        target_kind: row.try_get("target_kind").map_err(map_db_err)?,
        target_id: row.try_get("target_id").map_err(map_db_err)?,
        details: json_from_db(&details)?,
        node: node
            .map(|n| n.parse::<NodeId>())
            .transpose()
            .map_err(|e| StoreError::Database(format!("corrupt audit node: {e}")))?,
    })
}

pub(crate) const COMMENT_COLUMNS: &str = "id, item_id, parent_id, user_id, author_name, \
     author_email, author_url, body, status, ip_hash, created_at";

/// Maps a row selected with [`COMMENT_COLUMNS`].
pub(crate) fn comment(row: &SqliteRow) -> Result<Comment, StoreError> {
    let id: Uuid = row.try_get("id").map_err(map_db_err)?;
    let item: Uuid = row.try_get("item_id").map_err(map_db_err)?;
    let parent: Option<Uuid> = row.try_get("parent_id").map_err(map_db_err)?;
    let user: Option<Uuid> = row.try_get("user_id").map_err(map_db_err)?;
    let status: String = row.try_get("status").map_err(map_db_err)?;
    Ok(Comment {
        id: CommentId::from_uuid(id),
        item_id: ContentId::from_uuid(item),
        parent_id: parent.map(CommentId::from_uuid),
        user_id: user.map(UserId::from_uuid),
        author_name: row.try_get("author_name").map_err(map_db_err)?,
        author_email: row.try_get("author_email").map_err(map_db_err)?,
        author_url: row.try_get("author_url").map_err(map_db_err)?,
        body: row.try_get("body").map_err(map_db_err)?,
        status: status
            .parse()
            .map_err(|e| StoreError::Database(format!("corrupt comment status: {e}")))?,
        ip_hash: row.try_get("ip_hash").map_err(map_db_err)?,
        created_at: ts(row.try_get("created_at").map_err(map_db_err)?)?,
    })
}
