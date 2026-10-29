//! [`DemoBatch`]: everything `ferrite seed demo` writes, in one transaction.
//!
//! **For the operator CLI only.** The batch wraps a single `Tx`. Each method does what
//! the matching service call does (the same validation, rows, events and action jobs)
//! minus the up-front capability check (`edit_own`, `manage_terms`, `manage_menus`,
//! `manage_settings`): the caller is the seeding admin, and `user` inserts any account.
//! Publishing still authorizes `actor`. Nothing but the `ferrite` binary's `seed demo`
//! may use it; it is `pub` only because that crate is a different one. The seed has the
//! same effects as before but commits all of them together or none. Dropping the batch,
//! or [`rollback`](DemoBatch::rollback), discards everything. Nothing here opens another
//! transaction: with SQLite's single writer that would wait for this one.

use std::sync::Arc;

use ferrite_domain::{
    Artifact, ContentId, ContentItem, Media, Menu, NewUser, Term, Timestamp, User,
};
use ferrite_store::{Store, Tx};
use serde_json::Value;

use crate::{
    Actor, ContentDraft, ContentService, CoreError, Permalinks, events, menu, options,
    taxonomy::create_term_in,
};

/// One transaction of demo writes (see the module docs).
pub struct DemoBatch {
    tx: Box<dyn Tx>,
    /// Only for `create_in`, which must use the batch's `tx` and never `self.store`:
    /// a second connection would wait for this transaction (SQLite's single writer).
    content: ContentService,
    /// The permalink rules pinned when the batch began (R10).
    rules: Arc<dyn Permalinks>,
}

impl DemoBatch {
    /// Begins the transaction, with `rules` pinned for every item written.
    pub async fn begin(
        store: Arc<dyn Store>,
        rules: Arc<dyn Permalinks>,
    ) -> Result<Self, CoreError> {
        let tx = store.begin().await?;
        let rules = rules.pin();
        Ok(Self {
            tx,
            content: ContentService::new(store, rules.clone()),
            rules,
        })
    }

    /// The user with `new.login`, inserted if missing. A new user needs no event: nothing
    /// cached refers to it.
    pub async fn user(&self, new: &NewUser) -> Result<User, CoreError> {
        if let Some(u) = self.tx.users().get_by_login(&new.login).await? {
            return Ok(u);
        }
        Ok(self.tx.users().insert(new).await?)
    }

    /// The term `slug` of `taxonomy`, created (with its invalidation) if missing.
    pub async fn term(&self, taxonomy: &str, slug: &str, name: &str) -> Result<Term, CoreError> {
        if let Some(t) = self.tx.terms().get_by_slug(taxonomy, slug).await? {
            return Ok(t);
        }
        let (term, keys) = create_term_in(
            self.tx.as_ref(),
            taxonomy,
            name,
            Some(slug.to_owned()),
            None,
            "",
        )
        .await?;
        self.tx.events().append(&events::invalidate(keys)).await?;
        Ok(term)
    }

    /// Creates `draft` and publishes it. With `published_at` the first publish time is
    /// backdated first (`publish` keeps an existing one).
    pub async fn publish(
        &self,
        actor: &Actor,
        draft: ContentDraft,
        published_at: Option<Timestamp>,
    ) -> Result<ContentItem, CoreError> {
        let item = self.create_draft(actor, draft, published_at).await?;
        self.publish_item(actor, item).await
    }

    /// A media item: creates `draft` (kind `media`), stores `artifacts` (unless their
    /// hashes exist) and the `media` row `row` makes for the new item's id, emits
    /// `post:<id>` (R11) and publishes it.
    pub async fn media_item(
        &self,
        actor: &Actor,
        draft: ContentDraft,
        artifacts: &[Artifact],
        row: impl FnOnce(ContentId) -> Media,
    ) -> Result<ContentItem, CoreError> {
        let item = self.create_draft(actor, draft, None).await?;
        for a in artifacts {
            self.tx.artifacts().insert_if_absent(a).await?;
        }
        let row = row(item.id);
        self.tx.media().insert(&row).await?;
        self.tx
            .events()
            .append(&events::invalidate([events::post_key(item.id)]))
            .await?;
        self.publish_item(actor, item).await
    }

    /// Replaces a menu (validated as `MenuService::put` does) and emits `menu:<name>`.
    pub async fn menu(&self, menu: &Menu) -> Result<(), CoreError> {
        menu::put_in(self.tx.as_ref(), menu).await?;
        self.tx
            .events()
            .append(&events::invalidate([events::menu_key(&menu.name)]))
            .await?;
        Ok(())
    }

    /// Sets a non-`permalink.*` option, as `OptionService::set` does.
    pub async fn option(
        &self,
        actor: &Actor,
        key: &str,
        value: &Value,
        autoload: bool,
    ) -> Result<(), CoreError> {
        options::set_in(self.tx.as_ref(), actor, key, value, autoload).await
    }

    /// Commits every write.
    pub async fn commit(self) -> Result<(), CoreError> {
        Ok(self.tx.commit().await?)
    }

    /// Discards every write.
    pub async fn rollback(self) -> Result<(), CoreError> {
        Ok(self.tx.rollback().await?)
    }

    async fn create_draft(
        &self,
        actor: &Actor,
        draft: ContentDraft,
        published_at: Option<Timestamp>,
    ) -> Result<ContentItem, CoreError> {
        let mut item = self
            .content
            .create_in(self.tx.as_ref(), self.rules.as_ref(), actor, draft)
            .await?;
        if let Some(at) = published_at {
            item.published_at = Some(at);
            item = self.tx.content().update(&item).await?;
            self.tx
                .events()
                .append(&events::invalidate([events::post_key(item.id)]))
                .await?;
        }
        Ok(item)
    }

    async fn publish_item(
        &self,
        actor: &Actor,
        item: ContentItem,
    ) -> Result<ContentItem, CoreError> {
        ContentService::publish_in(self.tx.as_ref(), self.rules.as_ref(), actor, item).await
    }
}
