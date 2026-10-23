//! Admin reads of content (lists with filters or search, single items, terms,
//! revisions) and revision restore. Every read takes the [`Actor`] and checks
//! `require_read(Content)` (`read_admin` plus the `content:read` scope, F16), so a token
//! without a content scope reads nothing.

use ferrite_domain::{
    ContentId, ContentItem, ContentOrder, ContentQuery, ContentStatus, Page, ParentFilter,
    Revision, RevisionId, SearchQuery, TermId, UserId,
};

use super::{ContentDraft, ContentService, Edit, WALKED};
use crate::{
    Actor, CoreError,
    actor::{Area, Capability, require, require_read},
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
    /// `Ok` when content type `kind` exists (`NotFound` otherwise) and `actor` may read
    /// content.
    pub async fn check_type(&self, actor: &Actor, kind: &str) -> Result<(), CoreError> {
        require_read(actor, Area::Content)?;
        self.store
            .read()
            .types()
            .get(kind)
            .await?
            .map(|_| ())
            .ok_or(CoreError::NotFound)
    }

    /// One page of the items `query` selects (needs `require_read(Content)`; any
    /// author's items, as in WordPress). With search text, the ids come from the search
    /// index (the query's type and statuses, best match first) and the items are loaded
    /// in that order; the search cursor is passed through. Without, it is a
    /// [`ContentQuery`] in `query.order`. A cursor that does not belong to the query's
    /// kind of list is `Invalid`.
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
                statuses,
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
        Ok(Page {
            items,
            next_cursor: hits.next_cursor,
        })
    }

    /// Item `id` in any status (`require_read(Content)`); missing is `NotFound`.
    pub async fn get_for(&self, actor: &Actor, id: ContentId) -> Result<ContentItem, CoreError> {
        require_read(actor, Area::Content)?;
        self.get(id).await
    }

    /// The terms of item `id`, in their stored order (`require_read(Content)`).
    pub async fn terms_of(&self, actor: &Actor, id: ContentId) -> Result<Vec<TermId>, CoreError> {
        require_read(actor, Area::Content)?;
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

    /// Up to `limit` (clamped to 1..=[`MAX_REVISIONS_LISTED`]) revisions of item `id`,
    /// newest first (`require_read(Content)`); a missing item is `NotFound`.
    pub async fn revisions(
        &self,
        actor: &Actor,
        id: ContentId,
        limit: u32,
    ) -> Result<Vec<Revision>, CoreError> {
        require_read(actor, Area::Content)?;
        let repos = self.store.read();
        repos.content().get(id).await?.ok_or(CoreError::NotFound)?;
        Ok(repos
            .revisions()
            .list(id, limit.clamp(1, MAX_REVISIONS_LISTED))
            .await?)
    }

    /// Revision `rid` of item `id` (`require_read(Content)`); a revision of another item
    /// is `NotFound`.
    pub async fn revision(
        &self,
        actor: &Actor,
        id: ContentId,
        rid: RevisionId,
    ) -> Result<Revision, CoreError> {
        require_read(actor, Area::Content)?;
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
