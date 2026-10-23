//! Admin reads of content (lists with filters or search, single items, terms,
//! revisions) and revision restore. Every read takes the [`Actor`] and checks
//! `require_read(Content)` (`read_admin` plus the `content:read` scope, F16), so a token
//! without a content scope reads nothing, and then [`can_read`] per item (R17, R23):
//! others' items that are not publicly visible now (unpublished, or published with a
//! future `published_at`) only for roles with `edit_others`. Revisions need more: the
//! actor's role must be able to edit the item ([`role_can_edit`], R26).

use ferrite_domain::{
    ContentId, ContentItem, ContentOrder, ContentQuery, ContentStatus, Page, ParentFilter,
    ReadableBy, Revision, RevisionId, SearchQuery, TermId, Timestamp, UserId,
};

use super::{ContentDraft, ContentService, Edit, WALKED};
use crate::{
    Actor, CoreError,
    actor::{Area, Capability, can, can_read, require, require_read, role_can_edit},
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
        let readable_by = (!can(actor.role, Capability::EditOthers)).then_some(ReadableBy {
            user: actor.user,
            now,
        });
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
}
