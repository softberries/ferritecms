//! Imported content items (P4, P6, P7, P8): explicit status, dates and author, the WP
//! slug where core allows it, and re-runs that respect local edits.
//!
//! An item goes through the checks of [`ContentService`](crate::ContentService)'s
//! create and update (type, field schema, relations, parent, body root, SEO, slug and
//! root-namespace rules) and gets what a service save writes: its row, terms, SEO meta,
//! one revision, its search entry, its invalidation keys (collected into the batch's
//! one event) and, when it lands published, the redirect cleanup of `publish`. It
//! enqueues **no** `post.saved`/`post.published` action jobs (P6); a scheduled item
//! gets its `content.publish_scheduled` job.

use std::collections::HashMap;

use ferrite_domain::{
    ContentId, ContentItem, ContentStatus, ImportMapKind, NewContent, TermId, Timestamp, UserId,
};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    ImportBatch,
    batch::{WP_ID_META, check_meta_key},
};
use crate::{
    CoreError, blocks,
    content::{
        EXCERPT_CHARS, RootNamespace, Row, SeoPatch, check_body, check_item, dedup, index_search,
        invalidation_keys, reclaim_path, record_moves, root_namespace, save_with_free_slug,
        set_terms, write_revision,
    },
    events, jobs,
    redirects::redirect_keys,
    slug,
};

/// The note on the revision an imported write makes.
const REVISION_NOTE: &str = "WordPress import";

/// One WXR item converted for the import. Parent, author and terms are already mapped
/// to FerriteCMS ids; `status` is already mapped from WP's (P7), with WP `future` as
/// [`Scheduled`](ContentStatus::Scheduled).
#[derive(Debug, Clone, PartialEq)]
pub struct ImportContent {
    pub wp_id: i64,
    /// SHA-256 of the item's IR record (P3, P4).
    pub hash: String,
    pub kind: String,
    pub title: String,
    /// The WP `post_name`; `None` (or blank) derives one from the title.
    pub slug: Option<String>,
    pub parent: Option<ContentId>,
    pub author: UserId,
    pub status: ContentStatus,
    /// `post_date_gmt`. Required for `Scheduled` (the time to publish at); `None` on a
    /// published item means `created_at`. Stored as given for the other statuses.
    pub published_at: Option<Timestamp>,
    pub created_at: Timestamp,
    /// `post_modified_gmt`; kept after every write of the import.
    pub updated_at: Timestamp,
    /// `None`: derived from the body (160 characters).
    pub excerpt: Option<String>,
    pub body: Value,
    pub fields: Value,
    pub menu_order: i32,
    /// The item's terms in this order; duplicates are ignored.
    pub terms: Vec<TermId>,
    pub seo: Option<SeoPatch>,
    /// Extra meta (`wp.<name>` and the like); the keys [`ImportBatch::meta`] accepts.
    pub meta: Vec<(String, Value)>,
}

/// What [`ImportBatch::content`] did with an item (P4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Written {
    /// Not mapped (or its mapped item is gone): created.
    Created,
    /// Mapped, changed in WordPress and not edited here since: updated in place.
    Updated,
    /// Mapped with the same hash: nothing written.
    Unchanged,
    /// Mapped and changed in WordPress, but edited here since the import wrote it:
    /// nothing written ("kept: edited locally" in the report).
    KeptEdited,
}

/// The outcome of one imported item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentOutcome {
    pub id: ContentId,
    pub result: Written,
    /// The slug actually used, when it differs from the WP slug (taken or reserved, so
    /// core's `-N` was appended, or normalised; P8). `None` when the WP slug was kept
    /// or there was none.
    pub slug_changed: Option<String>,
    /// A WP `future` item whose date had already passed: published at that date (P7).
    pub schedule_past: bool,
}

/// Status and publish times as written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Dates {
    status: ContentStatus,
    published_at: Option<Timestamp>,
    scheduled_at: Option<Timestamp>,
    schedule_past: bool,
}

impl ImportBatch<'_> {
    /// Writes one imported item (P4, P7, P8):
    ///
    /// - not mapped, or its mapped item is gone: created ([`Written::Created`]);
    /// - mapped with the same `hash`: nothing written ([`Written::Unchanged`]);
    /// - mapped with another `hash`, and the item is still at the version the import
    ///   left it at: updated in place ([`Written::Updated`]), its type unchanged
    ///   (`Invalid` otherwise); redirects for a published item whose path moved are
    ///   made as by [`ContentService::update`](crate::ContentService::update);
    /// - otherwise (edited here since): nothing written ([`Written::KeptEdited`]).
    ///
    /// A write stores `c.status` directly (`private` too). `Scheduled` with a date
    /// ahead gets `scheduled_at` and its `content.publish_scheduled` job; with a date
    /// passed it is published at that date and `schedule_past` is set; without a date
    /// it is `Invalid`. `created_at` and `updated_at` are kept as given and the author
    /// is `c.author` (an unknown one is `InvalidField { field: "author" }`).
    ///
    /// The WP slug is kept when free; a taken or reserved one gets core's `-N` (as a
    /// generated slug would, never an error) and is reported in `slug_changed`. The
    /// meta [`WP_ID_META`] (`c.wp_id`) and `c.meta` are written; the map row
    /// (`content`, `c.wp_id`) records `c.hash` and the version written.
    pub async fn content(&mut self, c: ImportContent) -> Result<ContentOutcome, CoreError> {
        let unwritten = |id: Uuid, result| ContentOutcome {
            id: ContentId::from_uuid(id),
            result,
            slug_changed: None,
            schedule_past: false,
        };
        let current = match self.lookup(ImportMapKind::Content, c.wp_id).await? {
            None => None,
            Some(mapped) if mapped.hash == c.hash => {
                return Ok(unwritten(mapped.id, Written::Unchanged));
            }
            Some(mapped) => {
                let item = self
                    .tx
                    .content()
                    .get(ContentId::from_uuid(mapped.id))
                    .await?
                    .ok_or(CoreError::NotFound)?;
                if item.version != mapped.version {
                    return Ok(unwritten(mapped.id, Written::KeptEdited));
                }
                Some(item)
            }
        };
        check_body(&c.body)?;
        if let Some(seo) = &c.seo {
            seo.check()?;
        }
        for (key, _) in &c.meta {
            check_meta_key(key)?;
        }
        if self.tx.users().get(c.author).await?.is_none() {
            return Err(CoreError::invalid_field("author", "unknown author"));
        }
        let dates = dates(c.status, c.published_at, c.created_at, Timestamp::now())?;
        // Sources of the redirects the write touched.
        let mut redirected = Vec::new();
        let (saved, old_terms, old_parent, moved, result) = match current {
            None => {
                let saved = self.insert(&c, dates).await?;
                (saved, Vec::new(), None, Vec::new(), Written::Created)
            }
            Some(current) => {
                let old_parent = current.parent_id;
                let (saved, old_terms, moved) =
                    self.update(current, &c, dates, &mut redirected).await?;
                (saved, old_terms, old_parent, moved, Written::Updated)
            }
        };
        let id = saved.id;
        let terms = dedup(&c.terms);
        if let Some(seo) = &c.seo {
            seo.apply(self.tx.as_ref(), id).await?;
        }
        self.tx.meta().set(id, WP_ID_META, &json!(c.wp_id)).await?;
        for (key, value) in &c.meta {
            self.tx.meta().set(id, key, value).await?;
        }
        let repos = self.tx.as_ref();
        write_revision(
            repos,
            &self.handle.actor,
            &saved,
            Some(REVISION_NOTE.to_owned()),
        )
        .await?;
        match saved.status {
            ContentStatus::Trash => repos.search().remove(id).await?,
            _ => index_search(repos, &saved).await?,
        }
        match (saved.status, saved.scheduled_at) {
            (ContentStatus::Published, _) => {
                redirected.extend(reclaim_path(repos, self.rules.as_ref(), &saved).await?);
            }
            (ContentStatus::Scheduled, Some(at)) => {
                // `None`: the same (item, instant) already has its job.
                repos
                    .jobs()
                    .enqueue(&jobs::publish_scheduled_job(id, at))
                    .await?;
            }
            _ => {}
        }
        self.keys.extend(invalidation_keys(
            &saved, &old_terms, &terms, old_parent, &moved,
        ));
        self.keys.extend(redirect_keys(redirected));
        self.remember(
            ImportMapKind::Content,
            c.wp_id,
            id.as_uuid(),
            &c.hash,
            saved.version,
        )
        .await?;
        if let Some(ids) = &mut self.content_wp_ids {
            ids.insert(id.as_uuid(), c.wp_id);
        }
        self.items += 1;
        let wp_slug = wp_slug(&c);
        Ok(ContentOutcome {
            id,
            result,
            slug_changed: wp_slug.filter(|s| *s != saved.slug).map(|_| saved.slug),
            schedule_past: dates.schedule_past,
        })
    }

    /// Replaces the body of imported item `id` (the link fix-up pass, P14). The item
    /// must still be at the version the import left it at (else `Conflict`, nothing
    /// written: it was edited here); an item this source's map does not hold is
    /// `NotFound`. The version goes up and the map row follows it, so the item still
    /// counts as not edited locally (P4); `updated_at`, the excerpt and the revision
    /// are kept. The search entry is rewritten and the item's keys join the event.
    pub async fn set_body(&mut self, id: ContentId, body: Value) -> Result<(), CoreError> {
        check_body(&body)?;
        let wp_id = self.content_wp_id(id).await?.ok_or(CoreError::NotFound)?;
        let row = self
            .tx
            .imports()
            .map_get(&self.handle.run.source, ImportMapKind::Content, wp_id)
            .await?
            .filter(|row| row.ferrite_id == id.as_uuid())
            .ok_or(CoreError::NotFound)?;
        let current = self
            .tx
            .content()
            .get(id)
            .await?
            .ok_or(CoreError::NotFound)?;
        if current.version != row.version {
            return Err(CoreError::Conflict {
                current_version: current.version,
            });
        }
        if current.body == body {
            return Ok(());
        }
        let mut next = current.clone();
        next.body = body;
        next.body_html = None;
        let repos = self.tx.as_ref();
        let mut saved = repos.content().update(&next).await?;
        repos
            .content()
            .set_updated_at(id, current.updated_at)
            .await?;
        saved.updated_at = current.updated_at;
        if saved.status != ContentStatus::Trash {
            index_search(repos, &saved).await?;
        }
        self.keys.insert(events::post_key(id));
        self.keys.insert(events::archive_key(&saved.kind));
        self.remember(
            ImportMapKind::Content,
            wp_id,
            id.as_uuid(),
            &row.hash,
            saved.version,
        )
        .await?;
        self.items += 1;
        Ok(())
    }

    /// Inserts a new item for `c`.
    async fn insert(&self, c: &ImportContent, dates: Dates) -> Result<ContentItem, CoreError> {
        let repos = self.tx.as_ref();
        let base = base_slug(c, None);
        let types = repos.types().list().await?;
        check_item(
            repos,
            &types,
            &c.kind,
            &c.fields,
            c.parent.map(|p| (p, None)),
        )
        .await?;
        let root = root_namespace(
            repos,
            self.rules.as_ref(),
            &types,
            &c.kind,
            c.parent,
            false,
            &base,
        )
        .await?;
        let row = Row::New(NewContent {
            id: ContentId::new(),
            kind: c.kind.clone(),
            status: dates.status,
            slug: String::new(),
            parent_id: c.parent,
            author_id: c.author,
            title: c.title.clone(),
            excerpt: excerpt(c),
            body: c.body.clone(),
            body_html: None,
            fields: c.fields.clone(),
            menu_order: c.menu_order,
            published_at: dates.published_at,
            scheduled_at: dates.scheduled_at,
            created_at: Some(c.created_at),
            updated_at: Some(c.updated_at),
        });
        let saved = save_with_free_slug(self.tx.as_ref(), row, &base, None, &root).await?;
        set_terms(repos, saved.id, &dedup(&c.terms)).await?;
        Ok(saved)
    }

    /// Rewrites `current` (still at its mapped version) with `c`; returns the saved item,
    /// its previous terms and the descendants whose path moved, and adds the sources of
    /// the redirects it touched to `redirected`.
    async fn update(
        &self,
        current: ContentItem,
        c: &ImportContent,
        dates: Dates,
        redirected: &mut Vec<String>,
    ) -> Result<(ContentItem, Vec<TermId>, Vec<ContentId>), CoreError> {
        if c.kind != current.kind {
            return Err(CoreError::Invalid("the content type cannot change".into()));
        }
        let repos = self.tx.as_ref();
        let id = current.id;
        // Under the same parent the item's own slug is not a conflict.
        let own = (current.parent_id == c.parent).then_some(current.slug.as_str());
        let base = base_slug(c, Some(&current.slug));
        let types = repos.types().list().await?;
        let new_parent = c
            .parent
            .filter(|_| c.parent != current.parent_id)
            .map(|p| (p, Some(id)));
        check_item(repos, &types, &c.kind, &c.fields, new_parent).await?;
        let old_term_rows = repos.terms().for_item(id).await?;
        let old_terms: Vec<TermId> = old_term_rows.iter().map(|t| t.id).collect();
        let root = if own == Some(base.as_str()) {
            RootNamespace::default()
        } else {
            root_namespace(
                repos,
                self.rules.as_ref(),
                &types,
                &c.kind,
                c.parent,
                false,
                &base,
            )
            .await?
        };
        let mut next = current.clone();
        next.status = dates.status;
        next.parent_id = c.parent;
        next.author_id = c.author;
        next.title = c.title.clone();
        next.excerpt = excerpt(c);
        next.body = c.body.clone();
        next.body_html = None;
        next.fields = c.fields.clone();
        next.menu_order = c.menu_order;
        next.published_at = dates.published_at;
        next.scheduled_at = dates.scheduled_at;
        let mut saved = save_with_free_slug(repos, Row::Existing(next), &base, own, &root).await?;
        repos.content().set_updated_at(id, c.updated_at).await?;
        saved.updated_at = c.updated_at;
        let terms = dedup(&c.terms);
        set_terms(repos, id, &terms).await?;
        let moved = record_moves(
            repos,
            self.rules.as_ref(),
            &current,
            old_term_rows,
            &saved,
            terms != old_terms,
            redirected,
        )
        .await?;
        Ok((saved, old_terms, moved))
    }

    /// The WP id the content map of this run's source holds for `id`, read once per
    /// batch.
    async fn content_wp_id(&mut self, id: ContentId) -> Result<Option<i64>, CoreError> {
        if self.content_wp_ids.is_none() {
            let rows = self
                .tx
                .imports()
                .map_list(&self.handle.run.source, ImportMapKind::Content)
                .await?;
            let ids: HashMap<Uuid, i64> =
                rows.into_iter().map(|r| (r.ferrite_id, r.wp_id)).collect();
            self.content_wp_ids = Some(ids);
        }
        Ok(self
            .content_wp_ids
            .as_ref()
            .and_then(|ids| ids.get(&id.as_uuid()).copied()))
    }
}

/// The WP slug of `c`, trimmed; `None` when blank.
fn wp_slug(c: &ImportContent) -> Option<&str> {
    c.slug.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

/// The slug to start from: the WP slug normalised as core does (slugified when core
/// refuses it), else `current` (an update keeps its slug), else one from the title.
/// Reserved words are not refused here: the save steps around them with `-N` (P8).
fn base_slug(c: &ImportContent, current: Option<&str>) -> String {
    match (wp_slug(c), current) {
        (Some(s), _) => slug::validate(s).unwrap_or_else(|_| slug::slugify(s)),
        (None, Some(current)) => current.to_owned(),
        (None, None) => slug::slugify(&c.title),
    }
}

fn excerpt(c: &ImportContent) -> String {
    c.excerpt
        .clone()
        .unwrap_or_else(|| blocks::excerpt(&c.body, EXCERPT_CHARS))
}

/// Status and publish times for `status` (P7): `Scheduled` needs `published_at` and is
/// published at it when it is not after `now`; `Published` without a date takes
/// `created_at`; the rest keep `published_at` as given.
fn dates(
    status: ContentStatus,
    published_at: Option<Timestamp>,
    created_at: Timestamp,
    now: Timestamp,
) -> Result<Dates, CoreError> {
    let plain = |status, published_at| Dates {
        status,
        published_at,
        scheduled_at: None,
        schedule_past: false,
    };
    Ok(match status {
        ContentStatus::Scheduled => {
            let at = published_at.ok_or_else(|| {
                CoreError::Invalid("a scheduled item needs its publish date".into())
            })?;
            if at > now {
                Dates {
                    status,
                    published_at: None,
                    scheduled_at: Some(at),
                    schedule_past: false,
                }
            } else {
                Dates {
                    schedule_past: true,
                    ..plain(ContentStatus::Published, Some(at))
                }
            }
        }
        ContentStatus::Published => plain(status, published_at.or(Some(created_at))),
        _ => plain(status, published_at),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(micros: i64) -> Timestamp {
        Timestamp::from_micros(micros).unwrap_or_else(|_| Timestamp::now())
    }

    #[test]
    fn dates_follow_p7() {
        let (now, past, future, created) = (at(1_000), at(500), at(2_000), at(100));
        let s = dates(ContentStatus::Scheduled, Some(future), created, now);
        assert_eq!(
            s.ok(),
            Some(Dates {
                status: ContentStatus::Scheduled,
                published_at: None,
                scheduled_at: Some(future),
                schedule_past: false,
            })
        );
        let missed = dates(ContentStatus::Scheduled, Some(past), created, now).ok();
        assert_eq!(
            missed,
            Some(Dates {
                status: ContentStatus::Published,
                published_at: Some(past),
                scheduled_at: None,
                schedule_past: true,
            })
        );
        assert!(dates(ContentStatus::Scheduled, None, created, now).is_err());
        let p = dates(ContentStatus::Published, None, created, now).ok();
        assert_eq!(p.and_then(|d| d.published_at), Some(created));
        let d = dates(ContentStatus::Draft, None, created, now).ok();
        assert_eq!(
            d.map(|d| (d.status, d.published_at)),
            Some((ContentStatus::Draft, None))
        );
        let pr = dates(ContentStatus::Private, Some(past), created, now).ok();
        assert_eq!(pr.and_then(|d| d.published_at), Some(past));
    }
}
