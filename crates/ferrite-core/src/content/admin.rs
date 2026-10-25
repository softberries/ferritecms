//! Admin reads of content (lists with filters or search, single items, terms,
//! revisions) and revision restore. Every read takes the [`Actor`] and checks
//! `require_read(Content)` (`read_admin` plus the `content:read` scope, F16), so a token
//! without a content scope reads nothing, and then [`can_read`] per item (R17, R23):
//! others' items that are not publicly visible now (unpublished, or published with a
//! future `published_at`) only for roles with `edit_others`. Revisions need more: the
//! actor's role must be able to edit the item ([`role_can_edit`], R26).
//!
//! The autosave slot (M2b P15) lives here too: one unsaved working copy per (item,
//! user), read like revisions (F12) and written with [`can_edit`]. It is not a save:
//! no revision, event, action job, search entry or cache invalidation.

use std::collections::HashMap;

use ferrite_domain::{
    Autosave, ContentId, ContentItem, ContentOrder, ContentQuery, ContentStatus, Page,
    ParentFilter, ReadableBy, Revision, RevisionId, SearchQuery, TermId, Timestamp, UserId,
};
use ferrite_store::{MAX_ID_FILTER, StoreError, validate_json};

use super::{ContentDraft, ContentService, Edit, WALKED};
use crate::{
    Actor, CoreError,
    actor::{Area, Capability, can, can_edit, can_read, require, require_read, role_can_edit},
    tx::finish,
};

/// Most revisions [`ContentService::revisions`] returns (also how many are kept).
pub const MAX_REVISIONS_LISTED: u32 = 50;

/// An admin list of one content type ([`ContentService::list`]).
#[derive(Debug, Clone, PartialEq)]
pub struct AdminQuery {
    pub kind: String,
    /// Empty: every status but trash.
    pub statuses: Vec<ContentStatus>,
    pub author: Option<UserId>,
    pub term: Option<TermId>,
    pub parent: ParentFilter,
    /// Full-text search. Cannot be combined with `author`, `term` or `parent`; results
    /// come by relevance (`order` is ignored). Blank text is no search.
    pub q: Option<String>,
    pub order: ContentOrder,
    /// 1 through [`ferrite_domain::MAX_PAGE`].
    pub limit: u32,
    /// `next_cursor` of the previous page of the same query.
    pub cursor: Option<String>,
}

impl AdminQuery {
    /// Every non-trash item of `kind`, newest update first, 50 per page.
    pub fn new(kind: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            statuses: Vec::new(),
            author: None,
            term: None,
            parent: ParentFilter::Any,
            q: None,
            order: ContentOrder::UpdatedDesc,
            limit: 50,
            cursor: None,
        }
    }

    /// The search text, if any (trimmed, blank = none).
    pub fn search_text(&self) -> Option<&str> {
        self.q.as_deref().map(str::trim).filter(|q| !q.is_empty())
    }
}

/// Who wrote an item or a revision, as the admin shows it ([`ContentService::authors`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorRef {
    pub id: UserId,
    pub login: String,
    pub display_name: String,
}

/// How many items of one type are in each status, as the actor may read them
/// ([`ContentService::counts`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StatusCounts {
    pub draft: u64,
    pub pending: u64,
    pub scheduled: u64,
    pub published: u64,
    pub private: u64,
    pub trash: u64,
}

/// What [`ContentService::put_autosave`] stores: the editor's working copy of an item.
/// `excerpt` `None` = derive from the body; `slug`, `parent_id`, `menu_order` and
/// `terms` `None` = unchanged (F11). Only text is checked (no NUL, no control
/// characters), never the field schema, the parent or the terms: a draft may be
/// incomplete, and the real save checks it all.
#[derive(Debug, Clone, PartialEq)]
pub struct AutosaveDraft {
    /// The item version the copy was made from.
    pub base_version: i64,
    pub title: String,
    pub excerpt: Option<String>,
    pub slug: Option<String>,
    pub parent_id: Option<ContentId>,
    pub menu_order: Option<i32>,
    pub terms: Option<Vec<TermId>>,
    pub body: serde_json::Value,
    pub fields: serde_json::Value,
}

/// `InvalidField` when `value` holds NUL or a control character (`multiline`: tab,
/// line feed and carriage return are allowed).
fn check_draft_text(field: &'static str, value: &str, multiline: bool) -> Result<(), CoreError> {
    let bad = value
        .chars()
        .any(|c| c.is_control() && !(multiline && matches!(c, '\t' | '\n' | '\r')));
    if bad {
        Err(CoreError::invalid_field(
            field,
            format!("{field} must not contain control characters"),
        ))
    } else {
        Ok(())
    }
}

/// `InvalidField` when the JSON `value` holds NUL in a string or a key.
fn check_draft_json(field: &'static str, value: &serde_json::Value) -> Result<(), CoreError> {
    validate_json(value)
        .map_err(|_| CoreError::invalid_field(field, format!("{field} must not contain NUL")))
}

/// The `readable_by` filter of an admin list (and of [`ContentService::counts`]): set
/// when `actor`'s role lacks `edit_others` (see [`can_read`]).
fn readable_by(actor: &Actor, now: Timestamp) -> Option<ReadableBy> {
    (!can(actor.role, Capability::EditOthers)).then_some(ReadableBy {
        user: actor.user,
        now,
    })
}

impl ContentService {
    /// One page of the items `query` selects (needs `require_read(Content)`). Without
    /// `edit_others` in its role, `actor` sees only items publicly visible now
    /// (published, `published_at <= now`; R23) and its own (any status), as WordPress's
    /// REST API does for contributors and authors; the filter is part of the query, so
    /// pages, cursors and sizes stay exact. With search text, the
    /// ids come from the search index (the query's type, statuses and the same
    /// visibility, best match first) and the items are loaded in that order, dropping any
    /// whose status or visibility changed in between; the search cursor is passed
    /// through. Without, it is a [`ContentQuery`] in `query.order`. A cursor that does not
    /// belong to the query's kind of list is `Invalid`.
    pub async fn list(
        &self,
        actor: &Actor,
        query: &AdminQuery,
    ) -> Result<Page<ContentItem>, CoreError> {
        require_read(actor, Area::Content)?;
        let statuses = if query.statuses.is_empty() {
            WALKED.to_vec()
        } else {
            query.statuses.clone()
        };
        let now = Timestamp::now();
        let readable_by = readable_by(actor, now);
        let repos = self.store.read();
        let Some(q) = query.search_text() else {
            return Ok(repos
                .content()
                .list(&ContentQuery {
                    kind: Some(query.kind.clone()),
                    statuses,
                    parent: query.parent,
                    term: query.term,
                    author: query.author,
                    readable_by,
                    order: query.order,
                    limit: query.limit,
                    cursor: query.cursor.clone(),
                    ..ContentQuery::default()
                })
                .await?);
        };
        if query.author.is_some() || query.term.is_some() || query.parent != ParentFilter::Any {
            return Err(CoreError::Invalid(
                "q cannot be combined with author, term or parent".into(),
            ));
        }
        let hits = repos
            .search()
            .search(&SearchQuery {
                q: q.to_owned(),
                kinds: vec![query.kind.clone()],
                statuses: statuses.clone(),
                readable_by,
                limit: query.limit,
                cursor: query.cursor.clone(),
            })
            .await?;
        // An empty id set short-circuits (M1c-1 R3).
        let items = if hits.items.is_empty() {
            Vec::new()
        } else {
            repos.content().get_many(&hits.items).await?
        };
        // The rows may have changed since the search read them.
        let items = items
            .into_iter()
            .filter(|i| {
                i.kind == query.kind && statuses.contains(&i.status) && can_read(actor, i, now)
            })
            .collect();
        Ok(Page {
            items,
            next_cursor: hits.next_cursor,
        })
    }

    /// Item `id` in any status, if `actor` may read it ([`can_read`] after
    /// `require_read(Content)`). Missing or not readable is `NotFound`, so the existence
    /// of others' items that are not publicly visible is not revealed.
    pub async fn get_for(&self, actor: &Actor, id: ContentId) -> Result<ContentItem, CoreError> {
        require_read(actor, Area::Content)?;
        let item = self.get(id).await?;
        if can_read(actor, &item, Timestamp::now()) {
            Ok(item)
        } else {
            Err(CoreError::NotFound)
        }
    }

    /// The terms of item `id`, in their stored order (as [`get_for`](Self::get_for)
    /// checks it).
    pub async fn terms_of(&self, actor: &Actor, id: ContentId) -> Result<Vec<TermId>, CoreError> {
        self.get_for(actor, id).await?;
        Ok(self
            .store
            .read()
            .terms()
            .for_item(id)
            .await?
            .into_iter()
            .map(|t| t.id)
            .collect())
    }

    /// Item `id` if `actor` may read its revisions: [`get_for`](Self::get_for), then
    /// [`role_can_edit`] (R26: revisions keep text removed from the item, so readers of
    /// a published item do not see them). Either failing is `NotFound`.
    async fn get_for_revisions(
        &self,
        actor: &Actor,
        id: ContentId,
    ) -> Result<ContentItem, CoreError> {
        let item = self.get_for(actor, id).await?;
        if role_can_edit(actor, &item) {
            Ok(item)
        } else {
            Err(CoreError::NotFound)
        }
    }

    /// Up to `limit` (clamped to 1..=[`MAX_REVISIONS_LISTED`]) revisions of item `id`,
    /// newest first, if `actor`'s role may edit the item (`NotFound` otherwise, see
    /// [`get_for_revisions`](Self::get_for_revisions)).
    pub async fn revisions(
        &self,
        actor: &Actor,
        id: ContentId,
        limit: u32,
    ) -> Result<Vec<Revision>, CoreError> {
        self.get_for_revisions(actor, id).await?;
        Ok(self
            .store
            .read()
            .revisions()
            .list(id, limit.clamp(1, MAX_REVISIONS_LISTED))
            .await?)
    }

    /// Revision `rid` of item `id`, if `actor`'s role may edit the item (as
    /// [`revisions`](Self::revisions)); a revision of another item is `NotFound`.
    pub async fn revision(
        &self,
        actor: &Actor,
        id: ContentId,
        rid: RevisionId,
    ) -> Result<Revision, CoreError> {
        self.get_for_revisions(actor, id).await?;
        self.store
            .read()
            .revisions()
            .get(rid)
            .await?
            .filter(|r| r.item_id == id)
            .ok_or(CoreError::NotFound)
    }

    /// Restores revision `rid` of item `id` (at `expected_version`): an
    /// [`update`](Self::update) with the revision's title, excerpt, body and fields and
    /// the item's current slug, parent, menu order and terms. Meta is not restored (P13).
    /// Same permissions, checks and events as `update`; the revision it writes has the
    /// note `restored from <rid>`. A revision of another item is `NotFound`.
    pub async fn restore_revision(
        &self,
        actor: &Actor,
        id: ContentId,
        rid: RevisionId,
        expected_version: i64,
    ) -> Result<ContentItem, CoreError> {
        // Writing needs `content:write` before anything is read.
        require(actor, Capability::EditOwn)?;
        // One set of rules for the whole write (R10).
        let rules = self.permalinks.pin();
        let tx = self.store.begin().await?;
        let done = async {
            let rev = tx
                .revisions()
                .get(rid)
                .await?
                .filter(|r| r.item_id == id)
                .ok_or(CoreError::NotFound)?;
            let current = tx.content().get(id).await?.ok_or(CoreError::NotFound)?;
            let terms = tx
                .terms()
                .for_item(id)
                .await?
                .into_iter()
                .map(|t| t.id)
                .collect();
            let draft = ContentDraft {
                kind: current.kind,
                title: rev.title,
                slug: None,
                parent_id: current.parent_id,
                excerpt: Some(rev.excerpt),
                body: rev.body,
                fields: rev.fields,
                menu_order: current.menu_order,
                terms,
            };
            self.update_in(
                tx.as_ref(),
                rules.as_ref(),
                actor,
                Edit {
                    id,
                    expected_version,
                    draft,
                    note: Some(format!("restored from {rid}")),
                },
            )
            .await
        }
        .await;
        finish(tx, done).await
    }
    /// The login and display name of each user in `ids` (needs `require_read(Content)`),
    /// keyed by id; repeated ids are looked up once, unknown ids are left out. Meant for
    /// the authors of items and revisions the actor has already read: logins and
    /// display names are public on author archives. One `get_many` per
    /// [`MAX_ID_FILTER`] ids.
    pub async fn authors(
        &self,
        actor: &Actor,
        ids: &[UserId],
    ) -> Result<HashMap<UserId, AuthorRef>, CoreError> {
        require_read(actor, Area::Content)?;
        let mut unique = ids.to_vec();
        unique.sort_unstable();
        unique.dedup();
        let users = self.store.read().users();
        let mut out = HashMap::with_capacity(unique.len());
        for chunk in unique.chunks(MAX_ID_FILTER) {
            for user in users.get_many(chunk).await? {
                out.insert(
                    user.id,
                    AuthorRef {
                        id: user.id,
                        login: user.login,
                        display_name: user.display_name,
                    },
                );
            }
        }
        Ok(out)
    }

    /// How many items of type `kind` are in each status, counting only what `actor` may
    /// read, with the same `readable_by` filter as [`list`](Self::list) (needs
    /// `require_read(Content)`; an unknown type is `NotFound`). One count per status.
    pub async fn counts(&self, actor: &Actor, kind: &str) -> Result<StatusCounts, CoreError> {
        require_read(actor, Area::Content)?;
        let repos = self.store.read();
        if repos.types().get(kind).await?.is_none() {
            return Err(CoreError::NotFound);
        }
        let readable_by = readable_by(actor, Timestamp::now());
        let content = repos.content();
        let count = async |status: ContentStatus| {
            content
                .count(&ContentQuery {
                    kind: Some(kind.to_owned()),
                    statuses: vec![status],
                    readable_by,
                    ..ContentQuery::default()
                })
                .await
        };
        Ok(StatusCounts {
            draft: count(ContentStatus::Draft).await?,
            pending: count(ContentStatus::Pending).await?,
            scheduled: count(ContentStatus::Scheduled).await?,
            published: count(ContentStatus::Published).await?,
            private: count(ContentStatus::Private).await?,
            trash: count(ContentStatus::Trash).await?,
        })
    }
}

impl ContentService {
    /// `actor`'s autosave of item `id`, if any. Read like revisions (F12):
    /// [`get_for`](Self::get_for) (`NotFound` when the item cannot be read), then
    /// [`role_can_edit`] (`Forbidden` otherwise), so a read-only token of a role that may
    /// edit the item still reads its own slot.
    pub async fn get_autosave(
        &self,
        actor: &Actor,
        id: ContentId,
    ) -> Result<Option<Autosave>, CoreError> {
        let item = self.get_for(actor, id).await?;
        if !role_can_edit(actor, &item) {
            let cap = if item.author_id == actor.user {
                Capability::EditOwn
            } else {
                Capability::EditOthers
            };
            return Err(CoreError::Forbidden(cap.as_str()));
        }
        Ok(self.store.read().autosaves().get(id, actor.user).await?)
    }

    /// Stores `draft` as `actor`'s autosave of item `id`, replacing the previous one
    /// (last write wins; `base_version` is kept, not compared). Needs [`can_edit`] on the
    /// item as stored now; a trashed item is `Invalid("restore first")`. Text is checked
    /// as [`AutosaveDraft`] says. Writes nothing else: no revision, event, action job,
    /// search entry or invalidation. The user's next real save of the item
    /// ([`update`](Self::update)) deletes it.
    pub async fn put_autosave(
        &self,
        actor: &Actor,
        id: ContentId,
        draft: AutosaveDraft,
    ) -> Result<Autosave, CoreError> {
        let item = self.get(id).await?;
        can_edit(actor, &item)?;
        if item.status == ContentStatus::Trash {
            return Err(CoreError::Invalid("restore first".into()));
        }
        check_draft_text("title", &draft.title, false)?;
        if let Some(excerpt) = &draft.excerpt {
            check_draft_text("excerpt", excerpt, true)?;
        }
        if let Some(slug) = &draft.slug {
            check_draft_text("slug", slug, false)?;
        }
        check_draft_json("body", &draft.body)?;
        check_draft_json("fields", &draft.fields)?;
        let autosave = Autosave {
            item_id: id,
            user_id: actor.user,
            base_version: draft.base_version,
            title: draft.title,
            excerpt: draft.excerpt,
            slug: draft.slug,
            parent_id: draft.parent_id,
            menu_order: draft.menu_order,
            terms: draft.terms,
            body: draft.body,
            fields: draft.fields,
            saved_at: Timestamp::now(),
        };
        match self.store.read().autosaves().put(&autosave).await {
            Ok(()) => Ok(autosave),
            // The item was deleted since it was read.
            Err(StoreError::ForeignKey { .. }) => Err(CoreError::NotFound),
            Err(e) => Err(e.into()),
        }
    }

    /// Deletes `actor`'s autosave of item `id` (needs [`can_edit`] on the item, trashed
    /// or not); `true` when there was one.
    pub async fn delete_autosave(&self, actor: &Actor, id: ContentId) -> Result<bool, CoreError> {
        let item = self.get(id).await?;
        can_edit(actor, &item)?;
        Ok(self.store.read().autosaves().delete(id, actor.user).await?)
    }
}
