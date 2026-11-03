//! The outbox (design 18 §4.1): an event row, one delivery per matching endpoint and the
//! first `webhook.deliver` job, written in the caller's `Tx`, so a write that rolls back
//! sends nothing. On a site without matching endpoints the only cost is one read.
//!
//! Payloads are thin (§3.2): ids and routing fields, no bodies, fields, e-mail addresses
//! or hashes.

use chrono::SecondsFormat;
use ferrite_domain::{
    Comment, CommentStatus, ContentItem, ContentStatus, ImportRun, MEDIA_KIND, Media, Timestamp,
    User, UserId, WebhookEndpoint, WebhookEventId, WebhookEventRow,
};
use ferrite_store::Repos;
use serde_json::{Map, Value, json};

use super::{MAX_COMMENT_BODY, MAX_TITLE, catalogue::matches, service::new_delivery};
use crate::{
    CoreError,
    jobs::{WEBHOOK_DELIVER_PRIORITY, webhook_deliver_job},
    permalink::{Permalinks, load_input},
};

/// An event to emit: its type, the content type it concerns (for the `content_types`
/// filter) and its `data`.
#[derive(Debug, Clone, PartialEq)]
pub struct NewWebhookEvent {
    pub kind: &'static str,
    pub content_type: Option<String>,
    pub data: Value,
}

fn ts(t: Option<Timestamp>) -> Value {
    t.map_or(Value::Null, |t| {
        Value::String(t.as_datetime().to_rfc3339_opts(SecondsFormat::Micros, true))
    })
}

fn cut(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

fn id_or_null<T: ToString>(v: Option<T>) -> Value {
    v.map_or(Value::Null, |v| Value::String(v.to_string()))
}

impl NewWebhookEvent {
    /// A `media.*` event (`kind` one of `media.uploaded`, `media.updated`,
    /// `media.trashed`, `media.restored`, `media.deleted`). No file URL.
    pub fn media(
        kind: &'static str,
        item: &ContentItem,
        media: &Media,
        actor: Option<UserId>,
    ) -> Self {
        Self {
            kind,
            content_type: None,
            data: json!({
                "id": item.id.to_string(),
                "title": cut(&item.title, MAX_TITLE),
                "alt": cut(&media.alt, MAX_TITLE),
                "mime": media.mime,
                "bytes": media.bytes,
                "width": media.width,
                "height": media.height,
                "original_name": cut(&media.original_name, MAX_TITLE),
                "actor": id_or_null(actor),
            }),
        }
    }

    /// A `comment.*` event on an item of type `item_type`; `previous` for
    /// `comment.moderated`. Never the e-mail, URL or address hash.
    pub fn comment(
        kind: &'static str,
        comment: &Comment,
        item_type: &str,
        previous: Option<CommentStatus>,
        actor: Option<UserId>,
    ) -> Self {
        let mut data = json!({
            "id": comment.id.to_string(),
            "item": {"id": comment.item_id.to_string(), "type": item_type},
            "parent": id_or_null(comment.parent_id),
            "status": comment.status.as_str(),
            "author_name": cut(&comment.author_name, MAX_TITLE),
            "user": id_or_null(comment.user_id),
            "body": cut(&comment.body, MAX_COMMENT_BODY),
            "created_at": ts(Some(comment.created_at)),
            "actor": id_or_null(actor),
        });
        if let (Some(p), Value::Object(m)) = (previous, &mut data) {
            m.insert("previous_status".into(), Value::String(p.as_str().into()));
        }
        Self {
            kind,
            content_type: None,
            data,
        }
    }

    /// A `user.*` event; `changed` for `user.updated`, `reassigned_to` for
    /// `user.deleted`. Never the e-mail address, hashes, TOTP or sessions.
    pub fn user(
        kind: &'static str,
        user: &User,
        changed: Option<Vec<&'static str>>,
        reassigned_to: Option<UserId>,
        actor: Option<UserId>,
    ) -> Self {
        let mut data = Map::new();
        data.insert("id".into(), Value::String(user.id.to_string()));
        data.insert("login".into(), Value::String(user.login.clone()));
        data.insert(
            "display_name".into(),
            Value::String(cut(&user.display_name, MAX_TITLE)),
        );
        data.insert("role".into(), Value::String(user.role.as_str().into()));
        data.insert("status".into(), Value::String(user.status.as_str().into()));
        data.insert("actor".into(), id_or_null(actor));
        if let Some(changed) = changed {
            data.insert("changed".into(), json!(changed));
        }
        if kind == "user.deleted" {
            data.insert("reassigned_to".into(), id_or_null(reassigned_to));
        }
        Self {
            kind,
            content_type: None,
            data: Value::Object(data),
        }
    }

    /// `import.finished` for `run` (its counters as `stats`).
    pub fn import(run: &ImportRun) -> Self {
        Self {
            kind: "import.finished",
            content_type: None,
            data: json!({
                "run": run.id.to_string(),
                "source": cut(&run.source, MAX_TITLE),
                "state": run.state.as_str(),
                "dry_run": run.dry_run,
                "stats": run.stats,
            }),
        }
    }
}

/// The content `data` (design 18 §3.2) of `item` with its permalink `path`; `public`
/// tells the renderer to add `url` (`site.url + path`) at delivery.
fn content_data(item: &ContentItem, path: String, actor: Option<UserId>) -> Value {
    let now = Timestamp::now();
    let mut data = json!({
        "id": item.id.to_string(),
        "type": item.kind,
        "status": item.status.as_str(),
        "title": cut(&item.title, MAX_TITLE),
        "slug": item.slug,
        "locale": item.locale,
        "translation_group": item.translation_group.to_string(),
        "path": path,
        "public": crate::site::is_public(item, now),
        "author": item.author_id.to_string(),
        "parent": id_or_null(item.parent_id),
        "version": item.version,
        "published_at": ts(item.published_at),
        "modified_at": ts(Some(item.updated_at)),
        "actor": id_or_null(actor),
    });
    if item.status == ContentStatus::Scheduled
        && let Value::Object(m) = &mut data
    {
        m.insert("scheduled_at".into(), ts(item.scheduled_at));
    }
    data
}

async fn matching(
    repos: &dyn Repos,
    kind: &str,
    content_type: Option<&str>,
) -> Result<Vec<WebhookEndpoint>, CoreError> {
    Ok(repos
        .webhooks()
        .active_for_emit()
        .await?
        .into_iter()
        .filter(|e| matches(e, kind, content_type))
        .collect())
}

async fn write(
    repos: &dyn Repos,
    endpoints: &[WebhookEndpoint],
    kind: &str,
    data: Value,
) -> Result<u32, CoreError> {
    let now = Timestamp::now();
    let event = WebhookEventRow {
        id: WebhookEventId::new(),
        kind: kind.to_owned(),
        data,
        created_at: now,
    };
    repos.webhooks().insert_event(&event).await?;
    for e in endpoints {
        let d = new_delivery(e, event.id, None, now);
        repos.webhooks().insert_delivery(&d).await?;
        repos
            .jobs()
            .enqueue(&webhook_deliver_job(
                d.id,
                1,
                None,
                WEBHOOK_DELIVER_PRIORITY,
            ))
            .await?;
    }
    Ok(u32::try_from(endpoints.len()).unwrap_or(u32::MAX))
}

/// Records `event` for every enabled endpoint of this install that subscribes to it and
/// queues its first attempts; returns how many deliveries were queued. Call it inside
/// the write's `Tx`.
pub async fn emit(repos: &dyn Repos, event: NewWebhookEvent) -> Result<u32, CoreError> {
    let endpoints = matching(repos, event.kind, event.content_type.as_deref()).await?;
    if endpoints.is_empty() {
        return Ok(0);
    }
    write(repos, &endpoints, event.kind, event.data).await
}

/// A `content.*` event for `item` (skipped for media items: `MediaService` emits
/// `media.*` itself). The permalink is computed with the write's pinned `permalinks`,
/// and only when an endpoint matches.
pub async fn emit_content(
    repos: &dyn Repos,
    permalinks: &dyn Permalinks,
    kind: &'static str,
    item: &ContentItem,
    actor: Option<UserId>,
) -> Result<u32, CoreError> {
    if item.kind == MEDIA_KIND {
        // Media emit `media.*` from `MediaService`; a permanent delete runs through the
        // content lifecycle, so it is turned into `media.deleted` here (the media row is
        // still there: this runs before the delete).
        if kind != "content.deleted" {
            return Ok(0);
        }
        let Some(media) = repos.media().get(item.id).await? else {
            return Ok(0);
        };
        return emit(
            repos,
            NewWebhookEvent::media("media.deleted", item, &media, actor),
        )
        .await;
    }
    let endpoints = matching(repos, kind, Some(&item.kind)).await?;
    if endpoints.is_empty() {
        return Ok(0);
    }
    let path = permalinks.path(&load_input(repos, item.clone()).await?);
    write(repos, &endpoints, kind, content_data(item, path, actor)).await
}

/// A `comment.*` event for `comment` (its item's type read only when an endpoint
/// matches).
pub async fn emit_comment(
    repos: &dyn Repos,
    kind: &'static str,
    comment: &Comment,
    previous: Option<CommentStatus>,
    actor: Option<UserId>,
) -> Result<u32, CoreError> {
    let endpoints = matching(repos, kind, None).await?;
    if endpoints.is_empty() {
        return Ok(0);
    }
    let item_type = repos
        .content()
        .get(comment.item_id)
        .await?
        .map(|i| i.kind)
        .unwrap_or_default();
    let event = NewWebhookEvent::comment(kind, comment, &item_type, previous, actor);
    write(repos, &endpoints, kind, event.data).await
}
