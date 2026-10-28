//! Import terms (P8): the WP slug where it is free, the parent as given.

use ferrite_domain::{ImportMapKind, TermId};

use super::ImportBatch;
use crate::{CoreError, taxonomy::create_term_in};

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

impl ImportBatch<'_> {
    /// The term of `t.wp_id`: the mapped term when it still exists (left unchanged),
    /// else a new term with the rules of
    /// [`TaxonomyService::create_term`](crate::TaxonomyService::create_term): the WP slug
    /// when free in the taxonomy, else `-2`, `-3`, … (P8). A new term adds its keys to the
    /// batch's event and writes the map row (`term`, `wp_id`).
    pub async fn term(&mut self, t: ImportTerm) -> Result<TermId, CoreError> {
        if let Some(mapped) = self.lookup(ImportMapKind::Term, t.wp_id).await? {
            return Ok(TermId::from_uuid(mapped.id));
        }
        let slug = Some(t.slug).filter(|s| !s.trim().is_empty());
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
        self.remember(ImportMapKind::Term, t.wp_id, term.id.as_uuid(), "", 0)
            .await?;
        Ok(term.id)
    }
}
