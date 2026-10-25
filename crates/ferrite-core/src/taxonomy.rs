//! Taxonomies and their terms.
//!
//! Terms: create, update, delete and list, with slugs unique per taxonomy (needs
//! `manage_terms`; reads `require_read(Terms)`, F16).
//!
//! Taxonomies: register, change and remove (needs `manage_settings`; reads
//! `require_read(Content)`, F16). The name rules, the permalink check inside the
//! transaction, the `types.changed` (`taxonomy:<name>`) + `invalidate [all]` events, the
//! audit rows (`taxonomy.create|update|delete`) and the accepted races are those of
//! [`ContentTypeService`](crate::ContentTypeService) (see `types.rs`).

use std::sync::Arc;

use ferrite_domain::{Taxonomy, Term, TermId, Timestamp};
use ferrite_store::{MAX_CONTENT_KINDS, Repos, Store, StoreError, Tx};
use serde_json::json;

use crate::{
    Actor, CoreError, MAX_PARENT_DEPTH,
    actor::{Area, Capability, require, require_read},
    events,
    permalink::term_path,
    redirects::{clear_redirects_from, record_move},
    site, slug,
    tx::finish,
    types::{
        Change, announce, begin_locked, check_label, check_name_free, check_permalinks,
        check_type_name, finish_locked,
    },
};

/// Unique index on (taxonomy, slug); a violation means another writer took the slug
/// between our lookup and our write.
const SLUG_CONSTRAINT: &str = "terms_taxonomy_slug_uq";
const SLUG_ATTEMPTS: usize = 5;
/// Manages terms of the registered taxonomies (needs `manage_terms`).
pub struct TaxonomyService {
    store: Arc<dyn Store>,
}

impl TaxonomyService {
    pub fn new(store: Arc<dyn Store>) -> Self {
        Self { store }
    }

    /// Every taxonomy, by name (`require_read(Content)`).
    pub async fn list_taxonomies(&self, actor: &Actor) -> Result<Vec<Taxonomy>, CoreError> {
        require_read(actor, Area::Content)?;
        Ok(self.store.read().taxonomies().list().await?)
    }

    /// Taxonomy `name` (`require_read(Content)`; missing: `NotFound`).
    pub async fn get_taxonomy(&self, actor: &Actor, name: &str) -> Result<Taxonomy, CoreError> {
        require_read(actor, Area::Content)?;
        self.store
            .read()
            .taxonomies()
            .get(name)
            .await?
            .ok_or(CoreError::NotFound)
    }

    /// Registers a taxonomy (needs `manage_settings`). The name follows the type name
    /// rules, the label is 1..=100 characters, every entry of `content_types` must be a
    /// registered type (duplicates are dropped, at most 20 remain), and `built_in` must
    /// be `false`.
    pub async fn create_taxonomy(
        &self,
        actor: &Actor,
        taxonomy: Taxonomy,
    ) -> Result<Taxonomy, CoreError> {
        require(actor, Capability::ManageSettings)?;
        check_type_name(&taxonomy.name)?;
        if taxonomy.built_in {
            return Err(CoreError::invalid_field(
                "built_in",
                "only the site's own taxonomies are built in",
            ));
        }
        let tax = Taxonomy {
            label: check_label(&taxonomy.label)?,
            content_types: check_content_types(&taxonomy.content_types)?,
            ..taxonomy
        };
        let (guard, tx) = begin_locked(self.store.as_ref()).await?;
        let done = async {
            check_name_free(tx.as_ref(), &tax.name, Change::Taxonomy).await?;
            check_types_exist(tx.as_ref(), &tax.content_types).await?;
            tx.taxonomies().insert(&tax).await?;
            check_permalinks(tx.as_ref(), true).await?;
            let details = json!({
                "label": tax.label,
                "hierarchical": tax.hierarchical,
                "content_types": tax.content_types,
            });
            announce(
                tx.as_ref(),
                actor.user,
                Change::Taxonomy,
                "create",
                &tax.name,
                details,
            )
            .await?;
            Ok(tax)
        }
        .await;
        finish_locked(guard, tx, done).await
    }

    /// Changes the label or the content types of taxonomy `name` (needs
    /// `manage_settings`; built-in taxonomies too). `None` keeps the stored value; the
    /// name and `hierarchical` cannot change. Missing: `NotFound`.
    pub async fn update_taxonomy(
        &self,
        actor: &Actor,
        name: &str,
        label: Option<String>,
        content_types: Option<Vec<String>>,
    ) -> Result<Taxonomy, CoreError> {
        require(actor, Capability::ManageSettings)?;
        let label = label.as_deref().map(check_label).transpose()?;
        let content_types = content_types
            .as_deref()
            .map(check_content_types)
            .transpose()?;
        let (guard, tx) = begin_locked(self.store.as_ref()).await?;
        let done = async {
            let mut tax = tx
                .taxonomies()
                .get(name)
                .await?
                .ok_or(CoreError::NotFound)?;
            if let Some(label) = label {
                tax.label = label;
            }
            if let Some(types) = content_types {
                check_types_exist(tx.as_ref(), &types).await?;
                tax.content_types = types;
            }
            tx.taxonomies().upsert(&tax).await?;
            check_permalinks(tx.as_ref(), false).await?;
            let details = json!({"label": tax.label, "content_types": tax.content_types});
            announce(
                tx.as_ref(),
                actor.user,
                Change::Taxonomy,
                "update",
                &tax.name,
                details,
            )
            .await?;
            Ok(tax)
        }
        .await;
        finish_locked(guard, tx, done).await
    }

    /// Removes taxonomy `name` (needs `manage_settings`). Built-in taxonomies are
    /// `Invalid("built-in taxonomy")`, one with terms `Invalid("taxonomy has terms")`.
    /// Missing: `NotFound`.
    pub async fn delete_taxonomy(&self, actor: &Actor, name: &str) -> Result<(), CoreError> {
        require(actor, Capability::ManageSettings)?;
        let (guard, tx) = begin_locked(self.store.as_ref()).await?;
        let done = async {
            let tax = tx
                .taxonomies()
                .get(name)
                .await?
                .ok_or(CoreError::NotFound)?;
            if tax.built_in {
                return Err(CoreError::Invalid("built-in taxonomy".into()));
            }
            if !tx.terms().list(&tax.name).await?.is_empty() {
                return Err(CoreError::Invalid("taxonomy has terms".into()));
            }
            tx.taxonomies().delete(&tax.name).await?;
            announce(
                tx.as_ref(),
                actor.user,
                Change::Taxonomy,
                "delete",
                &tax.name,
                json!({}),
            )
            .await
        }
        .await;
        finish_locked(guard, tx, done).await
    }

    /// Every term of `taxonomy` (`require_read(Terms)`; unknown taxonomy: `NotFound`).
    pub async fn list_terms_for(
        &self,
        actor: &Actor,
        taxonomy: &str,
    ) -> Result<Vec<Term>, CoreError> {
        require_read(actor, Area::Terms)?;
        self.list_terms(taxonomy).await
    }

    /// Term `id` (`require_read(Terms)`; missing: `NotFound`).
    pub async fn get_term(&self, actor: &Actor, id: TermId) -> Result<Term, CoreError> {
        require_read(actor, Area::Terms)?;
        self.store
            .read()
            .terms()
            .get(id)
            .await?
            .ok_or(CoreError::NotFound)
    }

    /// Lists all terms of `taxonomy` (unknown taxonomy: `NotFound`). No actor: for the
    /// public site and system paths; the API uses [`list_terms_for`](Self::list_terms_for).
    pub async fn list_terms(&self, taxonomy: &str) -> Result<Vec<Term>, CoreError> {
        let repos = self.store.read();
        if repos.taxonomies().get(taxonomy).await?.is_none() {
            return Err(CoreError::NotFound);
        }
        Ok(repos.terms().list(taxonomy).await?)
    }

    /// Creates a term (an unknown `taxonomy` is `NotFound`). `slug = None` derives it
    /// from `name`; a slug that is taken in the
    /// taxonomy gets `-2`, `-3`, … (also caller slugs). `parent` must be a term of the
    /// same taxonomy.
    ///
    /// Finding a free slug lists the whole taxonomy, so it is O(n) in the number of its
    /// terms (the store has no prefix lookup for terms). A concurrent create that takes
    /// the same slug is retried, up to 5 attempts. Only hierarchical taxonomies accept a
    /// parent. Redirects from the new term's archive path (either slash variant) are
    /// deleted in the same transaction, as publishing does for content: they would
    /// outrank the archive once it has a public item.
    pub async fn create_term(
        &self,
        actor: &Actor,
        taxonomy: &str,
        name: &str,
        slug: Option<String>,
        parent: Option<TermId>,
        description: &str,
    ) -> Result<Term, CoreError> {
        require(actor, Capability::ManageTerms)?;
        let name = check_name(name)?;
        let base = match &slug {
            Some(s) => slug::validate(s)?,
            None => slug::slugify(&name),
        };
        let tx = self.store.begin().await?;
        let done = async {
            let tax = get_taxonomy(tx.as_ref(), taxonomy).await?;
            check_flat(&tax, parent)?;
            if let Some(parent) = parent {
                check_parent_taxonomy(tx.as_ref(), taxonomy, parent).await?;
            }
            let term = Term {
                id: TermId::new(),
                taxonomy: taxonomy.to_owned(),
                slug: String::new(),
                name,
                description: description.to_owned(),
                parent_id: parent,
            };
            let saved = save_with_free_slug(tx.as_ref(), term, &base, false).await?;
            // The archive path is the term's now; a redirect there would outrank it.
            clear_redirects_from(tx.as_ref(), &term_path(&tax.name, &saved.slug)).await?;
            emit(tx.as_ref(), &tax, &saved).await?;
            Ok(saved)
        }
        .await;
        finish(tx, done).await
    }

    /// Replaces name, description and parent of term `id`; `slug = None` keeps the
    /// current slug. The taxonomy cannot change. A new parent must be in the same
    /// taxonomy and must not be the term itself or one of its descendants (the chain is
    /// walked up to 64 levels, only when the parent changes). Under READ COMMITTED two
    /// concurrent updates can still form a cycle together (plan ruling P2).
    ///
    /// When the slug changes and the term had a public archive (a public type in its
    /// taxonomy and a public item with the term, so no redirect reveals a term the site
    /// does not show, R7), a `301` from the old archive path to the new one is stored in
    /// the same transaction with the content-move rules: redirects to the old path are
    /// re-targeted (no chains), redirects from the new path (either slash variant) are
    /// deleted, and the `redirect:` keys are emitted. Any other slug change still deletes
    /// the redirects from the new path (either slash variant; they would outrank the
    /// archive once it has a public item), as [`create_term`](Self::create_term) does.
    pub async fn update_term(
        &self,
        actor: &Actor,
        id: TermId,
        name: &str,
        slug: Option<String>,
        parent: Option<TermId>,
        description: &str,
    ) -> Result<Term, CoreError> {
        require(actor, Capability::ManageTerms)?;
        let name = check_name(name)?;
        let explicit = slug.as_deref().map(slug::validate).transpose()?;
        let tx = self.store.begin().await?;
        let done = async {
            let current = tx.terms().get(id).await?.ok_or(CoreError::NotFound)?;
            let tax = get_taxonomy(tx.as_ref(), &current.taxonomy).await?;
            check_flat(&tax, parent)?;
            if let Some(new_parent) = parent
                && parent != current.parent_id
            {
                check_parent_taxonomy(tx.as_ref(), &current.taxonomy, new_parent).await?;
                check_cycle(tx.as_ref(), id, new_parent).await?;
            }
            let base = explicit.unwrap_or_else(|| current.slug.clone());
            let old_slug = current.slug.clone();
            let next = Term {
                name,
                description: description.to_owned(),
                parent_id: parent,
                ..current
            };
            let saved = save_with_free_slug(tx.as_ref(), next, &base, true).await?;
            if saved.slug != old_slug {
                let new_path = term_path(&tax.name, &saved.slug);
                if had_archive(tx.as_ref(), &tax, id).await? {
                    // Also clears the redirects from the new path.
                    record_move(tx.as_ref(), &term_path(&tax.name, &old_slug), &new_path).await?;
                } else {
                    clear_redirects_from(tx.as_ref(), &new_path).await?;
                }
            }
            emit(tx.as_ref(), &tax, &saved).await?;
            Ok(saved)
        }
        .await;
        finish(tx, done).await
    }

    /// Deletes a term. A term with child terms is `Invalid("has children")`; links to
    /// content items go with it. A missing term is `NotFound`.
    pub async fn delete_term(&self, actor: &Actor, id: TermId) -> Result<(), CoreError> {
        require(actor, Capability::ManageTerms)?;
        let tx = self.store.begin().await?;
        let done = async {
            let term = tx.terms().get(id).await?.ok_or(CoreError::NotFound)?;
            let tax = get_taxonomy(tx.as_ref(), &term.taxonomy).await?;
            let siblings = tx.terms().list(&term.taxonomy).await?;
            if siblings.iter().any(|t| t.parent_id == Some(id)) {
                return Err(CoreError::Invalid("has children".into()));
            }
            tx.terms().delete(id).await?;
            emit(tx.as_ref(), &tax, &term).await?;
            Ok(())
        }
        .await;
        finish(tx, done).await
    }
}

/// Whether term `id` of `tax` has a public archive: a public type in the taxonomy and
/// a public item with the term (one content query at most).
async fn had_archive(repos: &dyn Repos, tax: &Taxonomy, id: TermId) -> Result<bool, CoreError> {
    let kinds = site::public_kinds(tax);
    if kinds.is_empty() {
        return Ok(false);
    }
    let mut q = site::published(Timestamp::now());
    q.kinds = kinds;
    q.term = Some(id);
    q.limit = 1;
    Ok(!repos.content().list(&q).await?.items.is_empty())
}

/// `types` without repeats; more than [`MAX_CONTENT_KINDS`] distinct types is
/// `InvalidField { field: "content_types" }` (a term archive lists at most that many).
fn check_content_types(types: &[String]) -> Result<Vec<String>, CoreError> {
    let types = dedup_names(types);
    if types.len() > MAX_CONTENT_KINDS {
        return Err(CoreError::invalid_field(
            "content_types",
            format!("a taxonomy applies to at most {MAX_CONTENT_KINDS} content types"),
        ));
    }
    Ok(types)
}

/// `names` without repeats, in their first order.
fn dedup_names(names: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(names.len());
    for n in names {
        if !out.contains(n) {
            out.push(n.clone());
        }
    }
    out
}

/// Every name in `types` is a registered content type.
async fn check_types_exist(repos: &dyn Repos, types: &[String]) -> Result<(), CoreError> {
    let known = repos.types().list().await?;
    match types.iter().find(|t| !known.iter().any(|k| &k.name == *t)) {
        Some(t) => Err(CoreError::invalid_field(
            "content_types",
            format!("unknown content type {t:?}"),
        )),
        None => Ok(()),
    }
}

/// Only hierarchical taxonomies have parents.
fn check_flat(tax: &Taxonomy, parent: Option<TermId>) -> Result<(), CoreError> {
    if parent.is_some() && !tax.hierarchical {
        return Err(CoreError::invalid_field(
            "parent_id",
            format!(
                "taxonomy {:?} is not hierarchical; terms cannot have a parent",
                tax.name
            ),
        ));
    }
    Ok(())
}

fn check_name(name: &str) -> Result<String, CoreError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(CoreError::invalid_field(
            "name",
            "term name must not be empty",
        ));
    }
    Ok(name.to_owned())
}

async fn get_taxonomy(repos: &dyn Repos, name: &str) -> Result<Taxonomy, CoreError> {
    repos
        .taxonomies()
        .get(name)
        .await?
        .ok_or(CoreError::NotFound)
}

/// The parent must exist and belong to `taxonomy`.
async fn check_parent_taxonomy(
    repos: &dyn Repos,
    taxonomy: &str,
    parent: TermId,
) -> Result<(), CoreError> {
    match repos.terms().get(parent).await? {
        None => Err(CoreError::invalid_field("parent_id", "unknown parent")),
        Some(p) if p.taxonomy != taxonomy => Err(CoreError::invalid_field(
            "parent_id",
            "parent belongs to another taxonomy",
        )),
        Some(_) => Ok(()),
    }
}

/// Walks up from `parent` and fails if `id` is on the way, or the chain is deeper than
/// [`MAX_PARENT_DEPTH`].
async fn check_cycle(repos: &dyn Repos, id: TermId, parent: TermId) -> Result<(), CoreError> {
    let mut at = Some(parent);
    for _ in 0..MAX_PARENT_DEPTH {
        let Some(cur) = at else {
            return Ok(());
        };
        if cur == id {
            return Err(CoreError::invalid_field(
                "parent_id",
                "parent would create a cycle",
            ));
        }
        at = repos
            .terms()
            .get(cur)
            .await?
            .ok_or_else(|| CoreError::invalid_field("parent_id", "unknown parent"))?
            .parent_id;
    }
    if at.is_none() {
        return Ok(());
    }
    Err(CoreError::invalid_field(
        "parent_id",
        format!("parent chain deeper than {MAX_PARENT_DEPTH}"),
    ))
}

/// Writes `term` (insert, or update when `existing`) with the first free slug derived
/// from `base`. Each attempt runs in a savepoint, so losing a slug race poisons only
/// that attempt (R18). The term's own current slug does not count as taken.
async fn save_with_free_slug(
    tx: &dyn Tx,
    mut term: Term,
    base: &str,
    existing: bool,
) -> Result<Term, CoreError> {
    let mut failed: Vec<String> = Vec::new();
    loop {
        let attempt = tx.begin_nested().await?;
        let mut taken: Vec<String> = attempt
            .terms()
            .list(&term.taxonomy)
            .await?
            .into_iter()
            .filter(|t| t.id != term.id)
            .map(|t| t.slug)
            .collect();
        taken.extend(failed.iter().cloned());
        term.slug = slug::next_free(base, &taken);
        let written = if existing {
            attempt.terms().update(&term).await
        } else {
            attempt.terms().insert(&term).await
        };
        match written {
            Ok(()) => {
                attempt.commit().await?;
                return Ok(term);
            }
            Err(StoreError::UniqueViolation { constraint }) if constraint == SLUG_CONSTRAINT => {
                tracing::debug!(slug = %term.slug, "term slug taken concurrently; retrying");
                attempt.rollback().await?;
                failed.push(term.slug.clone());
                if failed.len() >= SLUG_ATTEMPTS {
                    return Err(StoreError::UniqueViolation { constraint }.into());
                }
            }
            Err(e) => return Err(e.into()),
        }
    }
}

/// `term:<id>` plus `archive:<type>` for every content type of the taxonomy. Pages that
/// show the term depend on `term:<id>`, so the items need no keys of their own.
async fn emit(repos: &dyn Repos, tax: &Taxonomy, term: &Term) -> Result<(), CoreError> {
    let keys = std::iter::once(events::term_key(term.id))
        .chain(tax.content_types.iter().map(|t| events::archive_key(t)));
    repos.events().append(&events::invalidate(keys)).await?;
    Ok(())
}
