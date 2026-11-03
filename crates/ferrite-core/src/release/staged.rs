//! Staged changes (design 15 §4.2, D1): checks without writing, and the draft a release
//! applies.

use ferrite_domain::{ContentItem, StagedChange};
use ferrite_store::Repos;

use crate::{
    Actor, ContentDraft, CoreError,
    content::{SeoPatch, check_body, check_item},
    slug,
};

/// Longest staged title (as the API's content title).
const MAX_TITLE: usize = 500;

/// The SEO patch of a staged change, parsed and checked.
fn seo_of(change: &StagedChange) -> Result<Option<SeoPatch>, CoreError> {
    change
        .seo
        .as_ref()
        .map(|v| {
            let p = SeoPatch::from_json(v)?;
            p.check()?;
            Ok(p)
        })
        .transpose()
}

/// The checks `update_in` runs that need no write (`check_body`, the SEO patch, the
/// type's field schema and relations, a changed parent, the slug syntax), for `item`.
pub(crate) async fn check(
    change: &StagedChange,
    repos: &dyn Repos,
    actor: &Actor,
    item: &ContentItem,
) -> Result<(), CoreError> {
    let title = change.title.trim();
    if title.is_empty() || title.chars().count() > MAX_TITLE {
        return Err(CoreError::invalid_field(
            "title",
            format!("1 to {MAX_TITLE} characters"),
        ));
    }
    check_body(&change.body)?;
    seo_of(change)?;
    if let Some(s) = &change.slug {
        slug::validate(s)?;
    }
    let types = repos.types().list().await?;
    let new_parent = change
        .parent_id
        .filter(|p| Some(*p) != item.parent_id)
        .map(|p| (p, Some(item.id)));
    check_item(repos, actor, &types, &item.kind, &change.fields, new_parent).await
}

/// The draft of `change` for an item of `kind`.
pub(crate) fn into_draft(change: &StagedChange, kind: &str) -> Result<ContentDraft, CoreError> {
    Ok(ContentDraft {
        kind: kind.to_owned(),
        title: change.title.clone(),
        slug: change.slug.clone(),
        parent_id: change.parent_id,
        excerpt: change.excerpt.clone(),
        body: change.body.clone(),
        fields: change.fields.clone(),
        menu_order: change.menu_order,
        terms: change.terms.clone(),
        seo: seo_of(change)?,
    })
}
