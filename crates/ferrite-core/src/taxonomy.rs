//! Terms: create, update, delete and list, with slugs unique per taxonomy.

use std::sync::Arc;

use ferrite_domain::{Taxonomy, Term, TermId};
use ferrite_store::{Repos, Store, StoreError, Tx};

use crate::{
    Actor, CoreError,
    actor::{Capability, require},
    events, slug,
    tx::finish,
};

/// Unique index on (taxonomy, slug); a violation means another writer took the slug
/// between our lookup and our write.
const SLUG_CONSTRAINT: &str = "terms_taxonomy_slug_uq";
const SLUG_ATTEMPTS: usize = 5;
/// Longest parent chain walked when checking a new parent for cycles.
const MAX_PARENT_DEPTH: usize = 64;

/// Manages terms of the registered taxonomies (needs `manage_terms`).
pub struct TaxonomyService {
    store: Arc<dyn Store>,
}

impl TaxonomyService {
    pub fn new(store: Arc<dyn Store>) -> Self {
        Self { store }
    }

    /// Lists all terms of `taxonomy` (unknown taxonomy: `NotFound`).
    pub async fn list_terms(&self, taxonomy: &str) -> Result<Vec<Term>, CoreError> {
        let repos = self.store.read();
        if repos.taxonomies().get(taxonomy).await?.is_none() {
            return Err(CoreError::NotFound);
        }
        Ok(repos.terms().list(taxonomy).await?)
    }

    /// Creates a term. `slug = None` derives it from `name`; a slug that is taken in the
    /// taxonomy gets `-2`, `-3`, … (also caller slugs). `parent` must be a term of the
    /// same taxonomy.
    ///
    /// Finding a free slug lists the whole taxonomy, so it is O(n) in the number of its
    /// terms (the store has no prefix lookup for terms). A concurrent create that takes
    /// the same slug is retried, up to 5 attempts. Only hierarchical taxonomies accept a
    /// parent.
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
            let next = Term {
                name,
                description: description.to_owned(),
                parent_id: parent,
                ..current
            };
            let saved = save_with_free_slug(tx.as_ref(), next, &base, true).await?;
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

/// Only hierarchical taxonomies have parents.
fn check_flat(tax: &Taxonomy, parent: Option<TermId>) -> Result<(), CoreError> {
    if parent.is_some() && !tax.hierarchical {
        return Err(CoreError::Invalid(format!(
            "taxonomy {:?} is not hierarchical; terms cannot have a parent",
            tax.name
        )));
    }
    Ok(())
}

fn check_name(name: &str) -> Result<String, CoreError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(CoreError::Invalid("term name must not be empty".into()));
    }
    Ok(name.to_owned())
}

async fn get_taxonomy(repos: &dyn Repos, name: &str) -> Result<Taxonomy, CoreError> {
    repos
        .taxonomies()
        .get(name)
        .await?
        .ok_or_else(|| CoreError::Invalid(format!("unknown taxonomy {name:?}")))
}

/// The parent must exist and belong to `taxonomy`.
async fn check_parent_taxonomy(
    repos: &dyn Repos,
    taxonomy: &str,
    parent: TermId,
) -> Result<(), CoreError> {
    match repos.terms().get(parent).await? {
        None => Err(CoreError::Invalid("unknown parent".into())),
        Some(p) if p.taxonomy != taxonomy => Err(CoreError::Invalid(
            "parent belongs to another taxonomy".into(),
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
            return Err(CoreError::Invalid("parent would create a cycle".into()));
        }
        at = repos
            .terms()
            .get(cur)
            .await?
            .ok_or_else(|| CoreError::Invalid("unknown parent".into()))?
            .parent_id;
    }
    if at.is_none() {
        return Ok(());
    }
    Err(CoreError::Invalid(format!(
        "parent chain deeper than {MAX_PARENT_DEPTH}"
    )))
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

/// `term:<id>` plus `archive:<type>` for every content type of the taxonomy.
async fn emit(repos: &dyn Repos, tax: &Taxonomy, term: &Term) -> Result<(), CoreError> {
    let keys = std::iter::once(events::term_key(term.id))
        .chain(tax.content_types.iter().map(|t| events::archive_key(t)));
    repos.events().append(&events::invalidate(keys)).await?;
    Ok(())
}
