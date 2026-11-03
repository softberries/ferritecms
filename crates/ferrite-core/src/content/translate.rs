//! Locales and translation groups of content items (design 13 §2.3–§2.4): the checks a
//! localized write runs, [`ContentService::translate`] and [`ContentService::relocate`].
//!
//! An item's locale and group are not [`ContentDraft`] fields (D7): they change only
//! through these operations.

use ferrite_domain::{
    ContentId, ContentItem, ContentQuery, ContentStatus, MEDIA_KIND, NewAudit, ParentFilter,
    TermId, Timestamp, locale::SiteLocales,
};
use ferrite_store::{Repos, Tx};
use serde_json::json;

use super::{
    ContentDraft, ContentService, Placement, RootNamespace, dedup, invalidation_keys, record_moves,
    root_namespace, seo::Seo, seo::SeoPatch, set_terms,
};
use crate::changes::{Capture, ObjectRef, Op};
use crate::{
    Actor, CoreError,
    actor::{Capability, can_edit, can_read, require},
    events, slug,
    tx::finish,
};

/// Something [`ContentService::translate`] could not carry over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranslateNote {
    /// The source's parent has no translation in the target locale: the draft is top
    /// level.
    ParentUntranslated,
    /// The source's term has no translation in the target locale: it was dropped.
    TermUntranslated(TermId),
}

impl TranslateNote {
    /// The note as the API and MCP spell it (`parent_untranslated`,
    /// `term_untranslated:<id>`).
    pub fn code(&self) -> String {
        match self {
            Self::ParentUntranslated => "parent_untranslated".to_owned(),
            Self::TermUntranslated(id) => format!("term_untranslated:{id}"),
        }
    }
}

/// A new translation and what it could not take over.
#[derive(Debug, Clone, PartialEq)]
pub struct Translated {
    pub item: ContentItem,
    pub notes: Vec<TranslateNote>,
}

/// A change of an item's locale and translation group ([`ContentService::relocate`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Relocation {
    /// The new locale; `None` keeps it.
    pub locale: Option<String>,
    /// `None` keeps the group, `Some(None)` leaves it (the item becomes its own group),
    /// `Some(Some(id))` joins the group of item `id`.
    pub translation_of: Option<Option<ContentId>>,
}

/// The group of item `of`, which a new or moved item of `kind` joins: it must be one
/// `actor` may read and of the same type (`InvalidField { field: "translation_of" }`
/// otherwise, a missing or unreadable item alike).
pub(crate) async fn join_target(
    repos: &dyn Repos,
    actor: &Actor,
    of: ContentId,
    kind: &str,
) -> Result<ContentId, CoreError> {
    let unknown = || CoreError::invalid_field("translation_of", "unknown item to translate");
    let target = repos
        .content()
        .get(of)
        .await?
        .filter(|t| can_read(actor, t, Timestamp::now()))
        .ok_or_else(unknown)?;
    if target.kind != kind {
        return Err(CoreError::invalid_field(
            "translation_of",
            format!("a translation must be of type {:?}", target.kind),
        ));
    }
    if target.status == ContentStatus::Trash {
        return Err(unknown());
    }
    Ok(target.translation_group)
}

/// [`CoreError::InUse`] naming the member when `group` already has one in `locale`
/// (other than `except`, the item that moves).
pub(crate) async fn check_group_free(
    repos: &dyn Repos,
    group: ContentId,
    locale: &str,
    except: Option<ContentId>,
) -> Result<(), CoreError> {
    let members = repos.content().group_members(&[group]).await?;
    if let Some(m) = members
        .iter()
        .find(|m| m.locale == locale && Some(m.id) != except)
    {
        return Err(CoreError::InUse(format!(
            "the translation into {locale} already exists: {}",
            m.id
        )));
    }
    Ok(())
}

/// `InvalidField { field: "parent_id" }` when `parent` exists in another locale than
/// `locale` (13 §2.3: parents share the item's locale).
pub(crate) async fn check_parent_locale(
    repos: &dyn Repos,
    parent: ContentId,
    locale: &str,
) -> Result<(), CoreError> {
    match repos.content().get(parent).await? {
        Some(p) if p.locale != locale => Err(CoreError::invalid_field(
            "parent_id",
            format!(
                "the parent is in {}, the item in {locale}: a parent must share the item's locale",
                p.locale
            ),
        )),
        _ => Ok(()),
    }
}

/// On a multilingual site, `InvalidField { field: "terms" }` when one of `terms` is in
/// another locale than `locale`. Nothing is read on a one-locale site.
pub(crate) async fn check_term_locales(
    repos: &dyn Repos,
    locales: &SiteLocales,
    terms: &[TermId],
    locale: &str,
) -> Result<(), CoreError> {
    if !locales.is_multilingual() {
        return Ok(());
    }
    for id in terms {
        if let Some(t) = repos.terms().get(*id).await?
            && t.locale != locale
        {
            return Err(CoreError::invalid_field(
                "terms",
                format!(
                    "the term {:?} is in {}, the item in {locale}: use its translation",
                    t.name, t.locale
                ),
            ));
        }
    }
    Ok(())
}

/// The id of the member of `group` in `locale`, if any.
async fn member_in(
    repos: &dyn Repos,
    group: ContentId,
    locale: &str,
) -> Result<Option<ContentId>, CoreError> {
    Ok(repos
        .content()
        .group_members(&[group])
        .await?
        .into_iter()
        .find(|m| m.locale == locale)
        .map(|m| m.id))
}

/// The translations into `locale` of `terms` (by term group), each `None` when it has
/// none, in order.
async fn term_translations(
    repos: &dyn Repos,
    terms: &[ferrite_domain::Term],
    locale: &str,
) -> Result<Vec<(TermId, Option<TermId>)>, CoreError> {
    if terms.is_empty() {
        return Ok(Vec::new());
    }
    let groups: Vec<TermId> = terms.iter().map(|t| t.translation_group).collect();
    let members = repos.terms().group_members(&groups).await?;
    Ok(terms
        .iter()
        .map(|t| {
            let to = if t.locale == locale {
                Some(t.id)
            } else {
                members
                    .iter()
                    .find(|m| m.translation_group == t.translation_group && m.locale == locale)
                    .map(|m| m.id)
            };
            (t.id, to)
        })
        .collect())
}

impl ContentService {
    /// Copies item `id` as a draft in `locale` (13 §2.4): needs `edit_own`; the source
    /// must be one `actor` may read (else `NotFound`); `locale` one of the site's locales
    /// (`InvalidField { field: "locale" }`), not the source's, and free in its group
    /// ([`CoreError::InUse`] naming the existing translation). The draft takes the
    /// title, excerpt, body, fields, menu order and SEO meta; the slug is the source's,
    /// allocated in `locale`; the parent is the parent's translation in `locale`, else
    /// none ([`TranslateNote::ParentUntranslated`]); each term its translation, else it
    /// is dropped ([`TranslateNote::TermUntranslated`]). The author is `actor`; the item
    /// joins the source's group. Audited as `content.translate`.
    pub async fn translate(
        &self,
        actor: &Actor,
        id: ContentId,
        locale: &str,
    ) -> Result<Translated, CoreError> {
        require(actor, Capability::EditOwn)?;
        let rules = self.permalinks.pin();
        let tx = self.store.begin().await?;
        let done = async {
            let mut cap = Capture::begin(tx.as_ref(), actor, Op::ContentTranslate, &[]).await?;
            let t = self
                .translate_in(tx.as_ref(), rules.as_ref(), actor, id, locale)
                .await?;
            // Here, not in `translate_in`: `seed demo` translates through that (R-M6-6-8).
            crate::webhooks::emit_content(
                tx.as_ref(),
                rules.as_ref(),
                "content.created",
                &t.item,
                Some(actor.user),
            )
            .await?;
            cap.created(ObjectRef::Content(t.item.id));
            cap.finish(tx.as_ref()).await?;
            Ok(t)
        }
        .await;
        finish(tx, done).await
    }

    /// [`translate`](Self::translate) inside the caller's `tx` with its pinned `rules`,
    /// without the capability check (the demo seed's batch).
    pub(crate) async fn translate_in(
        &self,
        tx: &dyn Tx,
        rules: &dyn crate::Permalinks,
        actor: &Actor,
        id: ContentId,
        locale: &str,
    ) -> Result<Translated, CoreError> {
        {
            let repos: &dyn Repos = tx;
            let source = repos
                .content()
                .get(id)
                .await?
                .filter(|s| can_read(actor, s, Timestamp::now()) && s.kind != MEDIA_KIND)
                .ok_or(CoreError::NotFound)?;
            if source.status == ContentStatus::Trash {
                return Err(CoreError::NotFound);
            }
            let locales = crate::locales::read(repos).await?;
            if !locales.contains(locale) {
                return Err(CoreError::invalid_field(
                    "locale",
                    format!("{locale:?} is not one of the site's locales"),
                ));
            }
            if source.locale == locale {
                return Err(CoreError::invalid_field(
                    "locale",
                    "the item is in this locale already",
                ));
            }
            check_group_free(repos, source.translation_group, locale, None).await?;
            let mut notes = Vec::new();
            let parent = match source.parent_id {
                None => None,
                Some(p) => {
                    let parent = repos.content().get(p).await?;
                    let found = match parent {
                        Some(parent) => member_in(repos, parent.translation_group, locale).await?,
                        None => None,
                    };
                    if found.is_none() {
                        notes.push(TranslateNote::ParentUntranslated);
                    }
                    found
                }
            };
            let source_terms = repos.terms().for_item(source.id).await?;
            let mut terms = Vec::new();
            for (from, to) in term_translations(repos, &source_terms, locale).await? {
                match to {
                    Some(t) => terms.push(t),
                    None => notes.push(TranslateNote::TermUntranslated(from)),
                }
            }
            let seo = Seo::load(repos, source.id).await?;
            let draft = ContentDraft {
                kind: source.kind.clone(),
                title: source.title.clone(),
                slug: Some(source.slug.clone()),
                parent_id: parent,
                excerpt: Some(source.excerpt.clone()),
                body: source.body.clone(),
                fields: source.fields.clone(),
                menu_order: source.menu_order,
                terms,
                seo: Some(SeoPatch {
                    title: seo.title.map(Some),
                    description: seo.description.map(Some),
                    canonical: seo.canonical.map(Some),
                    robots: (!seo.robots.is_empty()).then_some(Some(seo.robots)),
                }),
            };
            let item = self
                .create_in(
                    tx,
                    rules,
                    actor,
                    draft,
                    Placement {
                        locale: Some(locale.to_owned()),
                        group: Some(source.translation_group),
                    },
                )
                .await?;
            repos
                .audit()
                .append(&NewAudit {
                    actor_id: Some(actor.user),
                    action: "content.translate".to_owned(),
                    target_kind: "content".to_owned(),
                    target_id: Some(item.id.to_string()),
                    details: json!({ "source": source.id.to_string(), "locale": locale }),
                })
                .await?;
            Ok(Translated { item, notes })
        }
    }

    /// Moves item `id` (at `expected_version`, else `Conflict`) to another locale or
    /// translation group (13 §2.3), with `edit` rights on it. The locale must be one of
    /// the site's; an item with children cannot change its locale (`Invalid`: change the
    /// children first), nor move away from its parent's; the target locale must be free
    /// in the (new) group ([`CoreError::InUse`]). The slug is allocated again in the new
    /// locale, the terms are mapped to their translations (untranslated ones dropped),
    /// and a published item whose path changes gets the usual redirect. Emits the
    /// item's keys and both groups; audited as `content.relocate`.
    pub async fn relocate(
        &self,
        actor: &Actor,
        id: ContentId,
        expected_version: i64,
        change: Relocation,
    ) -> Result<ContentItem, CoreError> {
        let rules = self.permalinks.pin();
        let tx = self.store.begin().await?;
        let done = async {
            super::live_edit_checks(tx.as_ref(), actor, id).await?;
            relocate_in(
                tx.as_ref(),
                rules.as_ref(),
                actor,
                id,
                expected_version,
                change,
            )
            .await
        }
        .await;
        finish(tx, done).await
    }

    /// A field update and then a [`relocate`](Self::relocate) in one transaction (the
    /// API's `PATCH` with `locale` or `translation_of`; review fix of R-1-16): either both
    /// are saved or neither. An empty `change` is a plain update.
    pub async fn update_and_relocate(
        &self,
        actor: &Actor,
        id: ContentId,
        expected_version: i64,
        draft: ContentDraft,
        change: Relocation,
    ) -> Result<ContentItem, CoreError> {
        let rules = self.permalinks.pin();
        let tx = self.store.begin().await?;
        let done = async {
            super::live_edit_checks(tx.as_ref(), actor, id).await?;
            let cap = Capture::begin(
                tx.as_ref(),
                actor,
                Op::ContentUpdate,
                &[ObjectRef::Content(id)],
            )
            .await?;
            let saved = self
                .update_in(
                    tx.as_ref(),
                    rules.as_ref(),
                    actor,
                    super::Edit {
                        id,
                        expected_version,
                        draft,
                        note: None,
                    },
                )
                .await?;
            let saved = if change == Relocation::default() {
                saved
            } else {
                relocate_in(
                    tx.as_ref(),
                    rules.as_ref(),
                    actor,
                    id,
                    saved.version,
                    change,
                )
                .await?
            };
            cap.finish(tx.as_ref()).await?;
            Ok(saved)
        }
        .await;
        finish(tx, done).await
    }
}

/// [`ContentService::relocate`] inside the caller's `tx` with its pinned `rules`.
async fn relocate_in(
    tx: &dyn Tx,
    rules: &dyn crate::Permalinks,
    actor: &Actor,
    id: ContentId,
    expected_version: i64,
    change: Relocation,
) -> Result<ContentItem, CoreError> {
    let repos: &dyn Repos = tx;
    let current = repos.content().get(id).await?.ok_or(CoreError::NotFound)?;
    can_edit(actor, &current)?;
    if current.version != expected_version {
        return Err(CoreError::Conflict {
            current_version: current.version,
        });
    }
    if current.kind == MEDIA_KIND {
        return Err(CoreError::Invalid("media is shared by every locale".into()));
    }
    let locales = crate::locales::read(repos).await?;
    let locale = change
        .locale
        .clone()
        .unwrap_or_else(|| current.locale.clone());
    if !locales.contains(&locale) {
        return Err(CoreError::invalid_field(
            "locale",
            format!("{locale:?} is not one of the site's locales"),
        ));
    }
    let moves_locale = locale != current.locale;
    if moves_locale {
        let children = repos
            .content()
            .count(&ContentQuery {
                parent: ParentFilter::Of(id),
                ..ContentQuery::default()
            })
            .await?;
        if children > 0 {
            return Err(CoreError::Invalid(
                "the item has children: change their locale first".into(),
            ));
        }
        if let Some(parent) = current.parent_id {
            super::check_parent_locale(repos, parent, &locale).await?;
        }
    }
    let group = match change.translation_of {
        None => current.translation_group,
        Some(None) => current.id,
        Some(Some(of)) => join_target(repos, actor, of, &current.kind).await?,
    };
    if group == current.translation_group && !moves_locale {
        return Ok(current);
    }
    check_group_free(repos, group, &locale, Some(id)).await?;
    let slug = if moves_locale {
        free_slug(repos, rules, &current, &locale).await?
    } else {
        current.slug.clone()
    };
    let old_term_rows = repos.terms().for_item(id).await?;
    let old_terms: Vec<TermId> = old_term_rows.iter().map(|t| t.id).collect();
    let terms: Vec<TermId> = if moves_locale {
        term_translations(repos, &old_term_rows, &locale)
            .await?
            .into_iter()
            .filter_map(|(_, to)| to)
            .collect()
    } else {
        old_terms.clone()
    };
    let terms = dedup(&terms);
    let saved = repos
        .content()
        .relocate(id, &locale, group, &slug, expected_version)
        .await?;
    if terms != old_terms {
        set_terms(repos, id, &terms).await?;
    }
    let mut redirected = Vec::new();
    let moved = record_moves(
        repos,
        rules,
        &current,
        old_term_rows,
        &saved,
        terms != old_terms,
        &mut redirected,
    )
    .await?;
    let mut keys = invalidation_keys(&saved, &old_terms, &terms, current.parent_id, &moved);
    keys.push(events::group_key(current.translation_group));
    keys.extend(crate::redirects::redirect_keys(redirected));
    repos.events().append(&events::invalidate(keys)).await?;
    repos
                .audit()
                .append(&NewAudit {
                    actor_id: Some(actor.user),
                    action: "content.relocate".to_owned(),
                    target_kind: "content".to_owned(),
                    target_id: Some(id.to_string()),
                    details: json!({
                        "from": { "locale": current.locale, "group": current.translation_group.to_string() },
                        "to": { "locale": saved.locale, "group": saved.translation_group.to_string() },
                    }),
                })
                .await?;
    crate::webhooks::emit_content(repos, rules, "content.updated", &saved, Some(actor.user))
        .await?;
    Ok(saved)
}

/// A free slug for `item` moving to `locale`: its slug, else `-N`, under the root rules
/// of the new locale.
async fn free_slug(
    repos: &dyn Repos,
    rules: &dyn crate::Permalinks,
    item: &ContentItem,
    locale: &str,
) -> Result<String, CoreError> {
    let types = repos.types().list().await?;
    let root: RootNamespace = root_namespace(
        repos,
        rules,
        &types,
        (&item.kind, locale),
        item.parent_id,
        false,
        &item.slug,
    )
    .await?;
    let mut taken = repos
        .content()
        .slugs_with_prefix(&item.kind, Some(locale), item.parent_id, &item.slug)
        .await?;
    if item.parent_id.is_none() {
        taken.extend(slug::RESERVED.iter().map(|s| (*s).to_owned()));
        taken.extend(root.names.iter().cloned());
        if root.year {
            taken.push(item.slug.clone());
        }
        for peer in &root.peers {
            taken.extend(
                repos
                    .content()
                    .slugs_with_prefix(peer, Some(locale), None, &item.slug)
                    .await?,
            );
        }
    } else {
        taken.extend(slug::CHILD_RESERVED.iter().map(|s| (*s).to_owned()));
    }
    // The lookup is in the target locale, where the item is not yet: its own row never
    // counts as taken.
    Ok(slug::next_free(&item.slug, &taken))
}
