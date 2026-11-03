//! The reverse writes of a revert (design 17 §7.3): each object back to a before-image,
//! as the reverting person, through the crate's own write paths.

use ferrite_domain::{
    Change, ContentId, ContentItem, ContentStatus, Menu, Redirect, Term, TermId, Timestamp, UserId,
};
use ferrite_store::{Repos, Tx};
use serde_json::Value;

use super::object::ObjectRef;
use super::snapshot::{
    CommentSnap, ContentSnap, OptionSnap, PluginSnap, RedirectSnap, TermSnap, ThemeSnap, TypeSnap,
};
use crate::{
    Actor, ContentDraft, ContentService, CoreError, TRASHED_FROM_META, TypePatch,
    actor::{Capability, require},
    content::{Action, Edit, Robots, SeoPatch, transition_in, webhook_type},
    events,
    permalink::Permalinks,
};

/// The note of the revision a reverted content edit writes.
const REVERT_NOTE: &str = "Reverted an AI change";

/// One object to put back.
#[derive(Debug, Clone)]
pub(crate) struct Restore {
    pub object: ObjectRef,
    /// The before-image of the oldest selected change; `None`: the object did not exist.
    pub target: Option<Value>,
    pub newest_seq: u32,
    pub oldest_seq: u32,
}

fn bad_image() -> CoreError {
    CoreError::Internal("change image does not match its kind")
}

fn parse<T: serde::de::DeserializeOwned>(v: &Value) -> Result<T, CoreError> {
    serde_json::from_value(v.clone()).map_err(|_| bad_image())
}

/// Whether a setting key needs the types lock (`permalink.*`, the locale options).
fn needs_types(key: &str) -> bool {
    crate::options::is_permalink(key) || crate::options::is_locale_key(key)
}

/// An option of a theme's or plugin's settings (written as stored, under the
/// extensions lock), not a site setting.
fn is_extension_option(key: &str) -> bool {
    key.starts_with("theme.") || key.starts_with("plugin.")
}

/// The locks a revert of `objects` takes (`types`, `extensions`), in that order.
pub(crate) fn locks_needed(objects: &[ObjectRef], _changes: &[Change]) -> (bool, bool) {
    let types = objects.iter().any(|o| match o {
        ObjectRef::Type(_) => true,
        ObjectRef::Option(k) => needs_types(k),
        _ => false,
    });
    let extensions = objects.iter().any(|o| match o {
        ObjectRef::ActiveTheme | ObjectRef::Plugin(_) => true,
        ObjectRef::Option(k) => is_extension_option(k),
        _ => false,
    });
    (types, extensions)
}

/// Who edited a content item last and when (its newest revision), for a
/// `changed_since` conflict; nothing for other kinds.
pub(crate) async fn last_editor(
    repos: &dyn Repos,
    object: &ObjectRef,
) -> Result<(Option<UserId>, Option<Timestamp>), CoreError> {
    match object {
        ObjectRef::Content(id) | ObjectRef::Media(id) => Ok(
            match repos.revisions().list(*id, 1).await?.into_iter().next() {
                Some(r) => (Some(r.author_id), Some(r.created_at)),
                None => (None, None),
            },
        ),
        _ => Ok((None, None)),
    }
}

/// Applies every restore in order; site settings go last in one batch, so cross-key
/// checks see the final state. A failure names the change (`Failed { seq }`).
pub(crate) async fn apply(
    svc: &ContentService,
    tx: &dyn Tx,
    rules: &dyn Permalinks,
    actor: &Actor,
    restores: &[Restore],
    notes: &mut Vec<(u32, String)>,
) -> Result<(), super::service::RevertError> {
    let fail =
        |seq: u32| move |error: CoreError| super::service::RevertError::Failed { seq, error };
    let mut settings: Vec<(String, Option<Value>)> = Vec::new();
    let mut settings_seq = 0;
    for r in restores {
        let res = match &r.object {
            ObjectRef::Content(id) => content(svc, tx, rules, actor, *id, r, false, notes).await,
            ObjectRef::Media(id) => content(svc, tx, rules, actor, *id, r, true, notes).await,
            ObjectRef::Term(id) => term(tx, actor, *id, r, notes).await,
            ObjectRef::Menu(name) => menu(tx, actor, name, r).await,
            ObjectRef::Type(name) => content_type(tx, actor, name, r).await,
            ObjectRef::Redirect(id) => redirect(tx, actor, *id, r).await,
            ObjectRef::Comment(id) => comment(tx, actor, *id, r).await,
            ObjectRef::Option(key) if is_extension_option(key) => {
                extension_option(tx, actor, key, r).await
            }
            ObjectRef::Option(key) => {
                let value = match &r.target {
                    Some(v) => Some(parse::<OptionSnap>(v)?.value),
                    None => None,
                };
                settings.push((key.clone(), value));
                settings_seq = settings_seq.max(r.newest_seq);
                Ok(())
            }
            ObjectRef::ActiveTheme => theme(tx, actor, r).await,
            ObjectRef::Plugin(name) => plugin(tx, actor, name, r).await,
        };
        res.map_err(fail(r.newest_seq))?;
    }
    if !settings.is_empty() {
        let run = async {
            require(actor, Capability::ManageSettings)?;
            if crate::options::set_many_in(tx, actor, &settings).await? {
                crate::types::invalidate_all(tx).await?;
            }
            Ok::<_, CoreError>(())
        };
        run.await.map_err(fail(settings_seq))?;
    }
    Ok(())
}

async fn current_item(repos: &dyn Repos, id: ContentId) -> Result<Option<ContentItem>, CoreError> {
    Ok(repos.content().get(id).await?)
}

async fn move_status(
    tx: &dyn Tx,
    rules: &dyn Permalinks,
    actor: &Actor,
    item: ContentItem,
    action: Action,
) -> Result<ContentItem, CoreError> {
    let item = transition_in(tx, rules, actor, item.id, item.version, action).await?;
    // The same webhook events as the public transitions (media items emit none here:
    // `emit_content` skips them, and the media paths below emit `media.*`).
    let kind = webhook_type(action);
    crate::webhooks::emit_content(tx, rules, kind, &item, Some(actor.user)).await?;
    Ok(item)
}

/// `media.trashed` / `media.restored` for a media item, as `MediaService` emits them.
async fn media_event(
    tx: &dyn Tx,
    actor: &Actor,
    kind: &'static str,
    item: &ContentItem,
) -> Result<(), CoreError> {
    if let Some(media) = tx.media().get(item.id).await? {
        let ev = crate::webhooks::NewWebhookEvent::media(kind, item, &media, Some(actor.user));
        crate::webhooks::emit(tx, ev).await?;
    }
    Ok(())
}

async fn trash(
    tx: &dyn Tx,
    rules: &dyn Permalinks,
    actor: &Actor,
    item: ContentItem,
    media: bool,
    from: Option<&str>,
) -> Result<ContentItem, CoreError> {
    let id = item.id;
    let was = item.status;
    let version = item.version;
    let item = ContentService::trash_in(tx, rules, actor, item, version).await?;
    crate::webhooks::emit_content(tx, rules, "content.trashed", &item, Some(actor.user)).await?;
    if media {
        media_event(tx, actor, "media.trashed", &item).await?;
        // As the media trash path: restore knows whether to publish again (P53).
        let from = from.map(str::to_owned).unwrap_or_else(|| {
            if was == ContentStatus::Published {
                ContentStatus::Published.as_str().to_owned()
            } else {
                ContentStatus::Draft.as_str().to_owned()
            }
        });
        tx.meta()
            .set(id, TRASHED_FROM_META, &Value::from(from))
            .await?;
    }
    Ok(item)
}

/// A content or media item back to `r.target` (design 17 §7.3).
#[allow(clippy::too_many_arguments)]
async fn content(
    svc: &ContentService,
    tx: &dyn Tx,
    rules: &dyn Permalinks,
    actor: &Actor,
    id: ContentId,
    r: &Restore,
    media: bool,
    notes: &mut Vec<(u32, String)>,
) -> Result<(), CoreError> {
    let Some(item) = current_item(tx, id).await? else {
        // Absent now and before: nothing to do (the conflict check let it through).
        return Ok(());
    };
    let Some(target) = &r.target else {
        // The assistant created it: trash, never delete (D7).
        if item.status != ContentStatus::Trash {
            trash(tx, rules, actor, item, media, None).await?;
        }
        return Ok(());
    };
    let t: ContentSnap = parse(target)?;
    let mut item = item;
    if item.status == ContentStatus::Trash && t.status != ContentStatus::Trash {
        item = ContentService::restore_in(tx, rules, actor, item.id, item.version).await?;
        crate::webhooks::emit_content(tx, rules, "content.restored", &item, Some(actor.user))
            .await?;
        if media {
            media_event(tx, actor, "media.restored", &item).await?;
            tx.meta().delete(id, TRASHED_FROM_META).await?;
        }
    }
    if item.status != ContentStatus::Trash && editable_differs(tx, &item, &t).await? {
        let draft = ContentDraft {
            kind: item.kind.clone(),
            title: t.title.clone(),
            slug: Some(t.slug.clone()),
            parent_id: t.parent_id,
            excerpt: Some(t.excerpt.clone()),
            body: t.body.clone(),
            fields: t.fields.clone(),
            menu_order: t.menu_order,
            terms: t.terms.clone(),
            seo: Some(seo_patch(&t)?),
        };
        item = svc
            .update_in(
                tx,
                rules,
                actor,
                Edit {
                    id,
                    expected_version: item.version,
                    draft,
                    note: Some(REVERT_NOTE.into()),
                },
            )
            .await?;
    }
    if (item.locale != t.locale || item.translation_group != t.translation_group)
        && !notes.iter().any(|(s, _)| *s == r.newest_seq)
    {
        notes.push((
            r.newest_seq,
            "the language or translation group was left as it is now".into(),
        ));
    }
    if media && let Some(alt) = &t.alt {
        let mut row = tx
            .media()
            .get_for_update(id)
            .await?
            .ok_or(CoreError::NotFound)?;
        if &row.alt != alt {
            row.alt = alt.clone();
            tx.media().update(&row).await?;
            tx.events()
                .append(&events::invalidate([events::post_key(id)]))
                .await?;
        }
    }
    status(tx, rules, actor, item, &t, media, r.newest_seq, notes).await?;
    Ok(())
}

/// Whether the members an update writes differ between `item` and `t`.
async fn editable_differs(
    repos: &dyn Repos,
    item: &ContentItem,
    t: &ContentSnap,
) -> Result<bool, CoreError> {
    let mut terms: Vec<TermId> = repos
        .terms()
        .for_item(item.id)
        .await?
        .into_iter()
        .map(|t| t.id)
        .collect();
    terms.sort();
    let seo = crate::content::Seo::load(repos, item.id).await?;
    let robots: Vec<String> = seo.robots.iter().map(|r| r.as_str().to_owned()).collect();
    Ok(item.title != t.title
        || item.slug != t.slug
        || item.parent_id != t.parent_id
        || item.excerpt != t.excerpt
        || item.body != t.body
        || item.fields != t.fields
        || item.menu_order != t.menu_order
        || terms != t.terms
        || seo.title != t.seo.title
        || seo.description != t.seo.description
        || seo.canonical != t.seo.canonical
        || robots != t.seo.robots)
}

fn seo_patch(t: &ContentSnap) -> Result<SeoPatch, CoreError> {
    let robots: Vec<Robots> = t
        .seo
        .robots
        .iter()
        .map(|r| serde_json::from_value(Value::from(r.as_str())).map_err(|_| bad_image()))
        .collect::<Result<_, _>>()?;
    Ok(SeoPatch {
        title: Some(t.seo.title.clone()),
        description: Some(t.seo.description.clone()),
        canonical: Some(t.seo.canonical.clone()),
        robots: Some((!robots.is_empty()).then_some(robots)),
    })
}

/// The status of `item` back to `t.status` through the lifecycle (design 17 §7.3).
#[allow(clippy::too_many_arguments)]
async fn status(
    tx: &dyn Tx,
    rules: &dyn Permalinks,
    actor: &Actor,
    mut item: ContentItem,
    t: &ContentSnap,
    media: bool,
    seq: u32,
    notes: &mut Vec<(u32, String)>,
) -> Result<(), CoreError> {
    use ContentStatus::*;
    let rescheduled =
        item.status == Scheduled && t.status == Scheduled && item.scheduled_at != t.scheduled_at;
    if item.status == t.status && !rescheduled {
        return Ok(());
    }
    if t.status == Trash {
        trash(tx, rules, actor, item, media, t.trashed_from.as_deref()).await?;
        return Ok(());
    }
    let live = |s: ContentStatus| matches!(s, Published | Scheduled | Private);
    match t.status {
        Published => {
            if item.status == Private {
                item = move_status(tx, rules, actor, item, Action::Unpublish).await?;
            }
            move_status(tx, rules, actor, item, Action::Publish).await?;
        }
        Draft => {
            if live(item.status) {
                move_status(tx, rules, actor, item, Action::Unpublish).await?;
            } else if item.status == Pending {
                // No pending -> draft transition: through the trash and back.
                let item = move_status(tx, rules, actor, item, Action::Trash).await?;
                move_status(tx, rules, actor, item, Action::Restore).await?;
            }
        }
        Pending => {
            if live(item.status) {
                item = move_status(tx, rules, actor, item, Action::Unpublish).await?;
            }
            move_status(tx, rules, actor, item, Action::Submit).await?;
        }
        Scheduled => {
            if live(item.status) {
                item = move_status(tx, rules, actor, item, Action::Unpublish).await?;
            }
            match t.scheduled_at.filter(|at| *at > Timestamp::now()) {
                Some(at) => {
                    move_status(tx, rules, actor, item, Action::Schedule(at)).await?;
                }
                None => notes.push((
                    seq,
                    "the scheduled time has passed: the item was left as a draft".into(),
                )),
            }
        }
        Private | Trash => notes.push((
            seq,
            format!("the status was left as {}", item.status.as_str()),
        )),
    }
    Ok(())
}

async fn term(
    tx: &dyn Tx,
    actor: &Actor,
    id: TermId,
    r: &Restore,
    notes: &mut Vec<(u32, String)>,
) -> Result<(), CoreError> {
    require(actor, Capability::ManageTerms)?;
    let current = tx.terms().get(id).await?;
    match (&r.target, current) {
        (None, None) => Ok(()),
        (None, Some(_)) => crate::taxonomy::delete_term_in(tx, id).await,
        (Some(target), current) => {
            let t: TermSnap = parse(target)?;
            match current {
                Some(cur) => {
                    if cur.locale != t.locale || cur.translation_group != t.translation_group {
                        notes.push((
                            r.newest_seq,
                            "the language or translation group was left as it is now".into(),
                        ));
                    }
                    crate::taxonomy::update_term_in(
                        tx,
                        id,
                        t.name,
                        Some(t.slug),
                        t.parent_id,
                        &t.description,
                    )
                    .await
                    .map(|_| ())
                }
                None => {
                    let term = Term {
                        id,
                        taxonomy: t.taxonomy,
                        slug: t.slug,
                        name: t.name,
                        description: t.description,
                        parent_id: t.parent_id,
                        locale: t.locale,
                        translation_group: t.translation_group,
                    };
                    crate::taxonomy::reinsert_term_in(tx, &term, &t.items).await?;
                    let left = t.items_total.saturating_sub(t.items.len() as u64);
                    if left > 0 {
                        notes.push((
                            r.newest_seq,
                            format!("{left} linked items were not kept and stay unlinked"),
                        ));
                    }
                    Ok(())
                }
            }
        }
    }
}

async fn menu(tx: &dyn Tx, actor: &Actor, name: &str, r: &Restore) -> Result<(), CoreError> {
    require(actor, Capability::ManageMenus)?;
    match &r.target {
        None => match tx.menus().get(name).await? {
            Some(_) => crate::menu::delete_in(tx, name).await,
            None => Ok(()),
        },
        Some(target) => {
            let menu: Menu = parse(target)?;
            let keys = crate::menu::put_in(tx, &menu).await?;
            tx.events().append(&events::invalidate(keys)).await?;
            Ok(())
        }
    }
}

async fn content_type(
    tx: &dyn Tx,
    actor: &Actor,
    name: &str,
    r: &Restore,
) -> Result<(), CoreError> {
    require(actor, Capability::ManageSettings)?;
    match &r.target {
        None => match tx.types().get(name).await? {
            Some(_) => crate::types::delete_in(tx, actor.user, name).await,
            None => Ok(()),
        },
        Some(target) => {
            let t: TypeSnap = parse(target)?;
            let patch = TypePatch {
                label: Some(t.label.clone()),
                has_archive: Some(t.has_archive),
                fields: Some(crate::fields::schema_of(&t.fields)?),
                // The snapshot holds no publish gate (M6-4): the current one stays.
                quality_gate: None,
            };
            crate::types::update_in(tx, actor.user, name, Some(t.label), &patch).await?;
            crate::types::invalidate_all(tx).await
        }
    }
}

async fn redirect(
    tx: &dyn Tx,
    actor: &Actor,
    id: ferrite_domain::RedirectId,
    r: &Restore,
) -> Result<(), CoreError> {
    require(actor, Capability::ManageRedirects)?;
    let before = match &r.target {
        Some(v) => {
            let t: RedirectSnap = parse(v)?;
            Some(Redirect {
                id,
                source_path: t.source_path,
                target: t.target,
                code: t.code,
                hits: 0,
                origin: t.origin,
                created_at: Timestamp::now(),
                last_hit_at: None,
            })
        }
        None => None,
    };
    crate::redirects::restore_in(tx, id, before.as_ref()).await
}

async fn comment(
    tx: &dyn Tx,
    actor: &Actor,
    id: ferrite_domain::CommentId,
    r: &Restore,
) -> Result<(), CoreError> {
    require(actor, Capability::ModerateComments)?;
    let status = match &r.target {
        // A reply the assistant wrote goes to the trash (D7).
        None => ferrite_domain::CommentStatus::Trash,
        Some(v) => parse::<CommentSnap>(v)?.status,
    };
    let previous = tx.comments().get(id).await?.map(|c| c.status);
    if let Some(c) = tx.comments().set_status(id, status).await? {
        tx.events()
            .append(&events::invalidate([events::post_key(c.item_id)]))
            .await?;
        if let Some(previous) = previous.filter(|p| *p != c.status) {
            let kind = "comment.moderated";
            crate::webhooks::emit_comment(tx, kind, &c, Some(previous), Some(actor.user)).await?;
        }
    }
    Ok(())
}

/// A theme's or plugin's settings option written back as stored: a sealed secret stays
/// sealed, never opened (D12).
async fn extension_option(
    tx: &dyn Tx,
    actor: &Actor,
    key: &str,
    r: &Restore,
) -> Result<(), CoreError> {
    let cap = if key.starts_with("theme.") {
        Capability::ManageThemes
    } else {
        Capability::ManagePlugins
    };
    require(actor, cap)?;
    match &r.target {
        None => tx.options().delete(key).await?,
        Some(v) => {
            let value = parse::<OptionSnap>(v)?.value;
            tx.options().set(key, &value, false).await?;
        }
    }
    tx.events().append(&events::options_changed(key)).await?;
    Ok(())
}

async fn theme(tx: &dyn Tx, actor: &Actor, r: &Restore) -> Result<(), CoreError> {
    require(actor, Capability::ManageThemes)?;
    let again = || {
        CoreError::Invalid("the theme that was active is gone: activate it on Appearance".into())
    };
    let Some(target) = &r.target else {
        // The built-in default was active (no row): Appearance activates it (D13).
        return Err(CoreError::Invalid(
            "the built-in theme was active: activate it on Appearance".into(),
        ));
    };
    let t: ThemeSnap = parse(target)?;
    let row = tx.themes().get(&t.name).await?.ok_or_else(again)?;
    if row.artifact_hash.as_str() != t.artifact {
        return Err(again());
    }
    crate::extensions::write_activate_theme(tx, &t.name, &row.artifact_hash, None).await
}

async fn plugin(tx: &dyn Tx, actor: &Actor, name: &str, r: &Restore) -> Result<(), CoreError> {
    require(actor, Capability::ManagePlugins)?;
    let Some(target) = &r.target else {
        return Err(CoreError::Invalid(
            "the plugin was not installed then".into(),
        ));
    };
    let t: PluginSnap = parse(target)?;
    let row = tx
        .plugins()
        .get(name)
        .await?
        .ok_or_else(|| CoreError::Invalid("the plugin is no longer installed".into()))?;
    if row.enabled != t.enabled {
        crate::extensions::write_plugin_enabled(tx, name, t.enabled, None).await?;
    }
    Ok(())
}
