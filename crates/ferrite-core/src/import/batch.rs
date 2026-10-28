//! [`ImportBatch`]: one fenced transaction of import writes with one invalidation (P6).

use std::{collections::BTreeSet, sync::Arc};

use ferrite_domain::{
    CommentId, ContentId, ContentItem, ImportMapKind, ImportMapRow, Menu, Taxonomy, Term, TermId,
    Timestamp, UserId,
};
use ferrite_store::{LockGuard, Repos, Store, Tx};
use serde_json::Value;
use uuid::Uuid;

use super::{ImportHandle, TAKEN_OVER, release};
use crate::{
    CoreError, NewType, Permalinks, TypePatch, events, fields, options,
    taxonomy::{check_new_taxonomy, create_taxonomy_in},
    types::{begin_locked, check_new_type, create_in, update_in},
};

/// More distinct keys than this make the batch's event `invalidate [all]` (P6, R2).
const MAX_EVENT_KEYS: usize = 1_000;
/// The meta key holding an imported item's WP id (a JSON number, P3); the one
/// `import.` key [`ImportBatch::meta`] accepts.
pub const WP_ID_META: &str = "import.wp.id";
/// Meta key prefixes core writes itself.
const RESERVED_META_PREFIXES: [&str; 2] = ["seo.", "import."];
/// A schema write on a batch that does not hold the `types` lock.
const NEEDS_SCHEMA_BATCH: &str =
    "content types, taxonomies and permalink options need a schema batch";

/// What [`ImportBatch::content_type`] did with a type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeWritten {
    /// The type did not exist: registered.
    Created,
    /// The type existed with a schema and gained fields it lacked.
    Updated,
    /// The type existed and was left as it is.
    Unchanged,
}

/// What the import map holds for a source row whose target still exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mapped {
    pub id: Uuid,
    /// The SHA-256 of the item's IR record when it was written (P4).
    pub hash: String,
    /// The item's version right after the import wrote it (P4).
    pub version: i64,
}

/// One transaction of import writes, opened by [`ImportHandle::batch`] (or
/// [`schema_batch`](ImportHandle::schema_batch)) after the fence. It collects the
/// invalidation keys of its writes and emits them as one `invalidate` event on
/// [`commit`](Self::commit) (`[all]` past 1 000 keys). Dropping it without commit rolls
/// everything back, map rows included. Callers keep it to at most
/// [`BATCH_ITEMS`](Self::BATCH_ITEMS) items ([`len`](Self::len)).
pub struct ImportBatch<'a> {
    pub(super) handle: &'a ImportHandle,
    // Declared before `types_lock`: dropped first, so the rollback starts before the
    // lock is freed.
    pub(super) tx: Box<dyn Tx>,
    pub(super) keys: BTreeSet<String>,
    pub(super) items: usize,
    /// The permalink rules pinned when the batch opened (R10).
    pub(super) rules: Arc<dyn Permalinks>,
    /// The `permalink.*` rows written, last value per key.
    permalinks: Vec<(String, Option<Value>)>,
    types_lock: Option<Box<dyn LockGuard>>,
}

impl<'a> ImportBatch<'a> {
    /// The most items one batch should write (P6).
    pub const BATCH_ITEMS: usize = 500;

    /// Begins the transaction (under the `types` lock when `schema`) and fences it.
    pub(super) async fn open(
        handle: &'a ImportHandle,
        store: &dyn Store,
        rules: Arc<dyn Permalinks>,
        schema: bool,
    ) -> Result<Self, CoreError> {
        let (types_lock, tx) = if schema {
            let (guard, tx) = begin_locked(store).await?;
            (Some(guard), tx)
        } else {
            (None, store.begin().await?)
        };
        let fenced = tx
            .imports()
            .fence(handle.run.id, &handle.run.token, Timestamp::now())
            .await;
        let batch = Self {
            handle,
            tx,
            keys: BTreeSet::new(),
            items: 0,
            rules,
            permalinks: Vec::new(),
            types_lock,
        };
        match fenced {
            Ok(true) => Ok(batch),
            Ok(false) => Err(batch.abort(CoreError::Changed(TAKEN_OVER)).await),
            Err(e) => Err(batch.abort(e.into()).await),
        }
    }

    /// Items written so far (users and terms created, types and taxonomies registered
    /// or changed, content items created or updated and bodies fixed up; later tasks add
    /// theirs).
    pub fn len(&self) -> usize {
        self.items
    }

    /// Whether nothing was written yet.
    pub fn is_empty(&self) -> bool {
        self.items == 0
    }

    /// The map row of (`kind`, `wp_id`) for this run's source, when its target still
    /// exists; a row whose target was deleted is stale and reads as `None` (P3). Menu
    /// rows are returned unchecked (menus are keyed by name, not by id).
    pub async fn lookup(
        &self,
        kind: ImportMapKind,
        wp_id: i64,
    ) -> Result<Option<Mapped>, CoreError> {
        let repos = self.tx.as_ref();
        let Some(row) = repos
            .imports()
            .map_get(&self.handle.run.source, kind, wp_id)
            .await?
        else {
            return Ok(None);
        };
        if !target_exists(repos, kind, row.ferrite_id).await? {
            return Ok(None);
        }
        Ok(Some(Mapped {
            id: row.ferrite_id,
            hash: row.hash,
            version: row.version,
        }))
    }

    /// Item `id` as this batch sees it (any status), for checks before a write that
    /// would refuse it (a menu entry's target must not be trashed).
    pub async fn get_item(&self, id: ContentId) -> Result<Option<ContentItem>, CoreError> {
        Ok(self.tx.content().get(id).await?)
    }

    /// Term `id` as this batch sees it (the slug it was imported under).
    pub async fn get_term(&self, id: TermId) -> Result<Option<Term>, CoreError> {
        Ok(self.tx.terms().get(id).await?)
    }

    /// Menu `name` as this batch sees it (to tell a menu edited or deleted here, or one
    /// the site already had).
    pub async fn get_menu(&self, name: &str) -> Result<Option<Menu>, CoreError> {
        Ok(self.tx.menus().get(name).await?)
    }

    /// The stored value of option `key` (to tell an option changed here since the import
    /// wrote it).
    pub async fn get_option(&self, key: &str) -> Result<Option<Value>, CoreError> {
        Ok(self.tx.options().get(key).await?.map(|o| o.value))
    }

    /// Records (or replaces) the map row of (`kind`, `wp_id`) for this run's source.
    pub async fn remember(
        &mut self,
        kind: ImportMapKind,
        wp_id: i64,
        id: Uuid,
        hash: &str,
        version: i64,
    ) -> Result<(), CoreError> {
        self.tx
            .imports()
            .map_put(&ImportMapRow {
                source: self.handle.run.source.clone(),
                kind,
                wp_id,
                ferrite_id: id,
                hash: hash.to_owned(),
                version,
            })
            .await?;
        Ok(())
    }

    /// Registers taxonomy `t` when no taxonomy of that name exists (the rules of
    /// [`TaxonomyService::create_taxonomy`](crate::TaxonomyService::create_taxonomy)).
    /// An existing one is kept as it is; the result is `true` when its `hierarchical`
    /// differs from `t`'s, for the report. Needs a schema batch (else `Internal`).
    pub async fn taxonomy(&mut self, t: Taxonomy) -> Result<bool, CoreError> {
        self.require_schema()?;
        if let Some(existing) = self.tx.taxonomies().get(&t.name).await? {
            return Ok(existing.hierarchical != t.hierarchical);
        }
        let tax = check_new_taxonomy(t)?;
        create_taxonomy_in(self.tx.as_ref(), self.handle.actor.user, &tax).await?;
        self.schema_changed();
        Ok(false)
    }

    /// Registers type `t` when absent (the rules of
    /// [`ContentTypeService::create`](crate::ContentTypeService::create)). An existing
    /// type with a schema gains the fields of `t` it lacks (by name, appended in `t`'s
    /// order); one without a schema accepts any fields already and is left alone, as is
    /// everything else about an existing type. Needs a schema batch (else `Internal`).
    pub async fn content_type(&mut self, t: NewType) -> Result<TypeWritten, CoreError> {
        self.require_schema()?;
        let repos = self.tx.as_ref();
        let actor = self.handle.actor.user;
        let Some(existing) = repos.types().get(&t.name).await? else {
            let ty = check_new_type(&t)?;
            create_in(repos, actor, &ty, t.fields.as_deref()).await?;
            self.schema_changed();
            return Ok(TypeWritten::Created);
        };
        let (Some(mut defs), Some(wanted)) = (fields::schema_of(&existing.fields)?, t.fields)
        else {
            return Ok(TypeWritten::Unchanged);
        };
        let before = defs.len();
        for def in wanted {
            if !defs.iter().any(|d| d.name == def.name) {
                defs.push(def);
            }
        }
        if defs.len() == before {
            return Ok(TypeWritten::Unchanged);
        }
        fields::check_schema(&defs)?;
        let patch = TypePatch {
            fields: Some(Some(defs)),
            ..TypePatch::default()
        };
        update_in(repos, actor, &existing.name, None, &patch).await?;
        self.schema_changed();
        Ok(TypeWritten::Updated)
    }

    /// Sets one meta value of `item` and adds the item's key to the batch's event.
    ///
    /// The key must not be empty, and must not start with a prefix core owns: `seo.`
    /// (written through the item's SEO fields) and `import.`, except the exact key
    /// [`WP_ID_META`] (the item's WP id, P3). Others are `InvalidKey { key, .. }`.
    pub async fn meta(
        &mut self,
        item: ContentId,
        key: &str,
        value: &Value,
    ) -> Result<(), CoreError> {
        check_meta_key(key)?;
        self.tx.meta().set(item, key, value).await?;
        self.keys.insert(events::post_key(item));
        Ok(())
    }

    /// Writes allowlisted settings with the rules of
    /// [`OptionService::set_many`](crate::OptionService::set_many) (`None` or `null`
    /// deletes). `permalink.*` keys need a schema batch (else `Internal`) and make the
    /// batch's event `[all]`.
    pub async fn options(&mut self, rows: &[(String, Option<Value>)]) -> Result<(), CoreError> {
        if options::touches_permalinks(rows) {
            self.require_schema()?;
        }
        if options::set_many_in(self.tx.as_ref(), &self.handle.actor, rows).await? {
            self.keys.insert(events::ALL_KEY.to_owned());
        }
        for (key, value) in rows.iter().filter(|(k, _)| options::is_permalink(k)) {
            self.permalinks.retain(|(k, _)| k != key);
            self.permalinks.push((key.clone(), value.clone()));
        }
        Ok(())
    }

    /// The `permalink.*` options this batch wrote (last value per key; `None` or `null`
    /// deletes). Read it before [`commit`](Self::commit): the caller swaps its rules
    /// once the batch ends, since a batch pins the rules it opened with.
    pub fn permalink_changes(&self) -> &[(String, Option<Value>)] {
        &self.permalinks
    }

    /// Appends the batch's one `invalidate` event (none when no key was collected) and
    /// commits, then releases the `types` lock of a schema batch. On error everything is
    /// rolled back.
    pub async fn commit(mut self) -> Result<(), CoreError> {
        let keys = std::mem::take(&mut self.keys);
        let appended = if keys.is_empty() {
            Ok(())
        } else {
            self.tx
                .events()
                .append(&events::invalidate(event_keys(keys)))
                .await
                .map(|_| ())
                .map_err(CoreError::from)
        };
        self.end(appended).await
    }

    /// Rolls everything back (a dry run, or a batch the caller abandons).
    pub async fn rollback(self) -> Result<(), CoreError> {
        let Self { tx, types_lock, .. } = self;
        let rolled = tx.rollback().await.map_err(CoreError::from);
        release(types_lock).await;
        rolled
    }

    /// Commits on `Ok`, rolls back on `Err` (returning it), then releases the lock.
    async fn end(self, result: Result<(), CoreError>) -> Result<(), CoreError> {
        let Self { tx, types_lock, .. } = self;
        let done = crate::tx::finish(tx, result).await;
        release(types_lock).await;
        done
    }

    /// Rolls back, releases the lock and hands back `error`.
    async fn abort(self, error: CoreError) -> CoreError {
        match self.end(Err(error)).await {
            Err(e) => e,
            Ok(()) => CoreError::Internal("a rolled-back batch reported success"),
        }
    }

    fn require_schema(&self) -> Result<(), CoreError> {
        if self.types_lock.is_none() {
            return Err(CoreError::Internal(NEEDS_SCHEMA_BATCH));
        }
        Ok(())
    }

    /// A type, taxonomy or permalink change: every page may change (`[all]`).
    fn schema_changed(&mut self) {
        self.items += 1;
        self.keys.insert(events::ALL_KEY.to_owned());
    }
}

/// Whether the target of a map row of `kind` still exists.
async fn target_exists(
    repos: &dyn Repos,
    kind: ImportMapKind,
    id: Uuid,
) -> Result<bool, CoreError> {
    Ok(match kind {
        ImportMapKind::User => repos.users().get(UserId::from_uuid(id)).await?.is_some(),
        ImportMapKind::Term => repos.terms().get(TermId::from_uuid(id)).await?.is_some(),
        ImportMapKind::Media => repos.media().get(ContentId::from_uuid(id)).await?.is_some(),
        ImportMapKind::Content => repos
            .content()
            .get(ContentId::from_uuid(id))
            .await?
            .is_some(),
        ImportMapKind::Comment => repos
            .comments()
            .get(CommentId::from_uuid(id))
            .await?
            .is_some(),
        ImportMapKind::Menu | ImportMapKind::Option => true,
    })
}

/// The keys of the batch's `invalidate` event: `[all]` when `all` is among them or
/// there are more than [`MAX_EVENT_KEYS`], else all of them.
fn event_keys(keys: BTreeSet<String>) -> Vec<String> {
    if keys.len() > MAX_EVENT_KEYS || keys.contains(events::ALL_KEY) {
        vec![events::ALL_KEY.to_owned()]
    } else {
        keys.into_iter().collect()
    }
}

/// See [`ImportBatch::meta`].
pub(super) fn check_meta_key(key: &str) -> Result<(), CoreError> {
    if key.is_empty() {
        return Err(CoreError::invalid_key(key, "meta key must not be empty"));
    }
    if key != WP_ID_META
        && let Some(prefix) = RESERVED_META_PREFIXES.iter().find(|p| key.starts_with(*p))
    {
        return Err(CoreError::invalid_key(
            key,
            format!("meta keys starting with {prefix:?} are reserved"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_keys_collapse_to_all() {
        let many: BTreeSet<String> = (0..=MAX_EVENT_KEYS).map(|i| format!("post:{i}")).collect();
        assert_eq!(event_keys(many), [events::ALL_KEY]);
        let limit: BTreeSet<String> = (0..MAX_EVENT_KEYS).map(|i| format!("post:{i}")).collect();
        assert_eq!(event_keys(limit).len(), MAX_EVENT_KEYS);
        let with_all: BTreeSet<String> = ["post:1".to_owned(), events::ALL_KEY.to_owned()].into();
        assert_eq!(event_keys(with_all), [events::ALL_KEY]);
    }

    #[test]
    fn meta_keys() {
        assert!(check_meta_key(WP_ID_META).is_ok());
        assert!(check_meta_key("wp.quotes").is_ok());
        for bad in ["", "seo.title", "import.wp.idx", "import.other"] {
            assert!(
                matches!(check_meta_key(bad), Err(CoreError::InvalidKey { .. })),
                "{bad}"
            );
        }
    }
}
