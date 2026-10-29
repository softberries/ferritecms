//! Import terms (P8): the WP slug where it is free, the parent as given.

use ferrite_domain::{ImportMapKind, TermId};

use super::ImportBatch;
use crate::{CoreError, slug, taxonomy::create_term_in};

/// A WXR term (`<wp:category>`, `<wp:tag>`, `<wp:term>`), its parent already mapped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportTerm {
    pub wp_id: i64,
    pub taxonomy: String,
    /// The WP slug (decoded); empty derives one from the name.
    pub slug: String,
    pub name: String,
    pub description: String,
    pub parent: Option<TermId>,
}

/// How [`ImportBatch::term`] found the term.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TermMatched {
    /// The term an earlier run mapped `wp_id` to.
    Mapped,
    /// An existing site term with the same taxonomy and slug.
    Slug,
    /// A new term.
    Created,
}

/// The term an import term maps to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TermOutcome {
    pub id: TermId,
    pub how: TermMatched,
}

impl ImportBatch<'_> {
    /// Maps a WXR term to a term of `t.taxonomy`, in this order:
    ///
    /// 1. the term an earlier run mapped `t.wp_id` to, when it still exists **in the same
    ///    taxonomy** (else the map row counts as stale);
    /// 2. an existing term of the taxonomy with the same slug (the WP slug normalised as
    ///    core does, or the slug of the name when empty), reused as it is;
    /// 3. a new term with the rules of
    ///    [`TaxonomyService::create_term`](crate::TaxonomyService::create_term) and the
    ///    WP slug (P8); its keys join the batch's event.
    ///
    /// Mapped and reused terms are left unchanged (name, description, parent). The map
    /// row (`term`, `wp_id`) is written in cases 2 and 3.
    pub async fn term(&mut self, t: ImportTerm) -> Result<TermOutcome, CoreError> {
        if let Some(mapped) = self.lookup(ImportMapKind::Term, t.wp_id).await? {
            let id = TermId::from_uuid(mapped.id);
            let same_taxonomy = self
                .tx
                .terms()
                .get(id)
                .await?
                .is_some_and(|term| term.taxonomy == t.taxonomy);
            if same_taxonomy {
                return Ok(TermOutcome {
                    id,
                    how: TermMatched::Mapped,
                });
            }
        }
        let slug = Some(t.slug).filter(|s| !s.trim().is_empty());
        let wanted = match &slug {
            Some(s) => slug::validate(s)?,
            None => slug::slugify(&t.name),
        };
        let outcome =
            if let Some(existing) = self.tx.terms().get_by_slug(&t.taxonomy, &wanted).await? {
                TermOutcome {
                    id: existing.id,
                    how: TermMatched::Slug,
                }
            } else {
                let (term, keys) = create_term_in(
                    self.tx.as_ref(),
                    &t.taxonomy,
                    &t.name,
                    slug,
                    t.parent,
                    &t.description,
                )
                .await?;
                self.keys.extend(keys);
                self.items += 1;
                TermOutcome {
                    id: term.id,
                    how: TermMatched::Created,
                }
            };
        self.remember(ImportMapKind::Term, t.wp_id, outcome.id.as_uuid(), "", 0)
            .await?;
        Ok(outcome)
    }
}
