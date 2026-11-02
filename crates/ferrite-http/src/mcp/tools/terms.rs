//! Taxonomy and term tools (design 12 §5.2): `taxonomies_list`, `terms_list`,
//! `term_create`, `term_update` and `term_delete`.
//!
//! Adapters over [`TaxonomyService`](ferrite_core::TaxonomyService) and the REST layer's
//! DTOs and merge rule (`api::taxonomies::merge_term`). Core checks every
//! permission (`content:read` for taxonomies, `terms:read` and `terms:write` for terms).
//! Taxonomies themselves are not created or changed here: that is site structure for
//! a person (design 12 §5.4). Core lists every term of a taxonomy at once, so
//! `terms_list` filters and pages that list by `page` and `per_page` and says the total.

use ferrite_domain::{Term, TermId};
use rmcp::{handler::server::wrapper::Parameters, model::CallToolResult, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use super::per_page;
use crate::{
    api::{
        dto::{
            content::nullable,
            types::{TaxonomyDto, TermDto},
        },
        taxonomies::{TermChange, merge_term},
    },
    mcp::{Caller, FerriteMcp, results},
};

// ---- inputs ------------------------------------------------------------------------------

/// The input of `terms_list`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListInput {
    /// The taxonomy's name: `category`, `tag` or another from taxonomies_list.
    pub taxonomy: String,
    /// Only terms whose name or slug holds this text, ignoring case.
    #[serde(default)]
    pub q: Option<String>,
    /// `root` for top-level terms, or the id of a parent term to list its direct
    /// children.
    #[serde(default)]
    pub parent: Option<String>,
    /// The page, from 1 (default 1).
    #[serde(default)]
    #[schemars(range(min = 1))]
    pub page: Option<u32>,
    /// Terms per page, 1 to 100 (default 20).
    #[serde(default)]
    #[schemars(range(min = 1, max = 100))]
    pub per_page: Option<u32>,
}

/// The input of `term_create`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateInput {
    /// The taxonomy's name: `category`, `tag` or another from taxonomies_list.
    pub taxonomy: String,
    /// The term's name, as shown to readers (not empty).
    pub name: String,
    /// The URL slug. Derived from the name when absent; a taken slug gets `-2`, `-3`,
    /// ...
    #[serde(default)]
    pub slug: Option<String>,
    /// The parent term's id (hierarchical taxonomies such as `category` only).
    #[serde(default)]
    pub parent: Option<Uuid>,
    /// A short description (themes may show it on the archive page).
    #[serde(default)]
    pub description: Option<String>,
}

/// The input of `term_update`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateInput {
    /// The term's id.
    pub id: Uuid,
    /// A new name. Leave out (or `null`) to keep it.
    #[serde(default)]
    pub name: Option<String>,
    /// A new slug (changing it of a term with a public archive adds a redirect from the
    /// old address). Leave out (or `null`) to keep it.
    #[serde(default)]
    pub slug: Option<String>,
    /// A new parent term id; `null` moves the term to the top level (unlike the other
    /// members, where `null` keeps the value). Leave out to keep it.
    #[serde(default, deserialize_with = "nullable")]
    #[schemars(with = "Option<Uuid>")]
    pub parent: Option<Option<Uuid>>,
    /// A new description; `""` clears it. Leave out (or `null`) to keep it.
    #[serde(default)]
    pub description: Option<String>,
}

/// A term named by id (`term_delete`).
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IdInput {
    /// The term's id.
    pub id: Uuid,
}

// ---- outputs -----------------------------------------------------------------------------

/// `taxonomies_list`.
#[derive(Debug, Serialize)]
pub struct TaxonomiesOut {
    pub items: Vec<TaxonomyDto>,
}

/// One page of `terms_list`.
#[derive(Debug, Serialize)]
pub struct TermsOut {
    pub items: Vec<TermDto>,
    /// How many terms match, over all pages.
    pub total: usize,
    pub page: u32,
    pub per_page: u32,
}

// ---- tools -------------------------------------------------------------------------------

#[tool_router(router = term_tools, vis = "pub(crate)")]
impl FerriteMcp {
    /// Lists the site's taxonomies (ways to group content, such as `category` and
    /// `tag`): name, label, whether terms nest (`hierarchical`) and which content types
    /// use them. Use the name with terms_list and term_create.
    #[tool(annotations(
        title = "List taxonomies",
        read_only_hint = true,
        open_world_hint = false
    ))]
    pub async fn taxonomies_list(&self, caller: Caller) -> CallToolResult {
        match self.api.taxonomy.list_taxonomies(caller.actor()).await {
            Ok(items) => {
                let items: Vec<TaxonomyDto> = items.into_iter().map(Into::into).collect();
                let names: Vec<&str> = items.iter().map(|t| t.name.as_str()).collect();
                let summary = format!(
                    "{} taxonom{}: {}.",
                    items.len(),
                    plural_y(items.len()),
                    names.join(", ")
                );
                results::ok(TaxonomiesOut { items }, summary)
            }
            Err(e) => results::from_core(e),
        }
    }

    /// Lists the terms of one taxonomy (for example every category), filtered by words
    /// in the name or slug (`q`) or by `parent`, in pages with the total. Each term has
    /// its id (what content_create and content_update take in `terms`), slug, name,
    /// description and parent id.
    #[tool(annotations(title = "List terms", read_only_hint = true, open_world_hint = false))]
    pub async fn terms_list(
        &self,
        caller: Caller,
        Parameters(input): Parameters<ListInput>,
    ) -> CallToolResult {
        let per = match per_page(input.per_page) {
            Ok(n) => n,
            Err(e) => return e,
        };
        let page = input.page.unwrap_or(1);
        if page == 0 {
            return results::invalid(Some("page"), "page starts at 1");
        }
        let parent = match input.parent.as_deref() {
            None => None,
            Some("root") => Some(None),
            Some(id) => match Uuid::parse_str(id) {
                Ok(id) => Some(Some(id)),
                Err(_) => {
                    return results::invalid(
                        Some("parent"),
                        "parent is `root` or the id of a parent term",
                    );
                }
            },
        };
        let terms = match self
            .api
            .taxonomy
            .list_terms_for(caller.actor(), &input.taxonomy, None)
            .await
        {
            Ok(t) => t,
            Err(e) => return results::from_core(e),
        };
        let q = input
            .q
            .as_deref()
            .map(|q| q.trim().to_lowercase())
            .filter(|q| !q.is_empty());
        let matching: Vec<Term> = terms
            .into_iter()
            .filter(|t| match parent {
                None => true,
                Some(p) => t.parent_id.map(|id| id.as_uuid()) == p,
            })
            .filter(|t| {
                q.as_deref().is_none_or(|q| {
                    t.name.to_lowercase().contains(q) || t.slug.to_lowercase().contains(q)
                })
            })
            .collect();
        let total = matching.len();
        let skip = (page as usize - 1).saturating_mul(per as usize);
        let items: Vec<TermDto> = matching
            .into_iter()
            .skip(skip)
            .take(per as usize)
            .map(Into::into)
            .collect();
        let summary = format!(
            "{} of {total} {} term{} (page {page}).",
            items.len(),
            input.taxonomy,
            if total == 1 { "" } else { "s" }
        );
        results::ok(
            TermsOut {
                items,
                total,
                page,
                per_page: per,
            },
            summary,
        )
    }

    /// Creates a term (a category, a tag, ...) in a taxonomy. The slug is derived from
    /// the name unless given. Only hierarchical taxonomies such as `category` take a
    /// `parent`. Check terms_list first so you do not create a duplicate. Returns the
    /// term with its `id`, which content_create and content_update take in `terms`.
    #[tool(annotations(
        title = "Create term",
        read_only_hint = false,
        destructive_hint = false,
        idempotent_hint = false,
        open_world_hint = false
    ))]
    pub async fn term_create(
        &self,
        caller: Caller,
        Parameters(input): Parameters<CreateInput>,
    ) -> CallToolResult {
        match self
            .api
            .taxonomy
            .create_term(
                caller.actor(),
                &input.taxonomy,
                &input.name,
                input.slug,
                input.parent.map(TermId::from_uuid),
                input.description.as_deref().unwrap_or_default(),
            )
            .await
        {
            Ok(term) => {
                self.audit(
                    &caller,
                    "term_create",
                    "term",
                    Some(term.id.to_string()),
                    json!({"taxonomy": term.taxonomy}),
                )
                .await;
                let summary = format!(
                    "Created the {} term {:?} (id {}, slug {}).",
                    term.taxonomy, term.name, term.id, term.slug
                );
                results::ok(TermDto::from(term), summary)
            }
            Err(e) => results::from_core(e),
        }
    }

    /// Changes a term's name, slug, parent or description; what you leave out is kept, and
    /// so is a name, slug or description sent as `null`. Only `parent: null` has a meaning of
    /// its own: it moves the term to the top level.
    /// Changing the slug of a term with a public archive adds a redirect from the old
    /// address. Returns the updated term.
    #[tool(annotations(
        title = "Update term",
        read_only_hint = false,
        destructive_hint = false,
        idempotent_hint = true,
        open_world_hint = false
    ))]
    pub async fn term_update(
        &self,
        caller: Caller,
        Parameters(input): Parameters<UpdateInput>,
    ) -> CallToolResult {
        if input.name.is_none()
            && input.slug.is_none()
            && input.parent.is_none()
            && input.description.is_none()
        {
            return results::invalid(None, "send at least one of name, slug, parent, description");
        }
        let change = TermChange {
            name: input.name,
            slug: input.slug,
            parent_id: input.parent,
            description: input.description,
        };
        match merge_term(
            &self.api,
            caller.actor(),
            TermId::from_uuid(input.id),
            change,
        )
        .await
        {
            Ok(term) => {
                self.audit(
                    &caller,
                    "term_update",
                    "term",
                    Some(term.id.to_string()),
                    json!({"taxonomy": term.taxonomy}),
                )
                .await;
                let summary = format!("Updated the term {:?} (slug {}).", term.name, term.slug);
                results::ok(TermDto::from(term), summary)
            }
            Err(e) => results::from_core(e),
        }
    }

    /// Deletes a term for good: content that had it loses the link (the content itself
    /// stays) and its archive page disappears. A term with child terms cannot be
    /// deleted; move or delete the children first. This cannot be undone, so ask the
    /// user first.
    #[tool(annotations(
        title = "Delete term",
        read_only_hint = false,
        destructive_hint = true,
        idempotent_hint = true,
        open_world_hint = false
    ))]
    pub async fn term_delete(
        &self,
        caller: Caller,
        Parameters(input): Parameters<IdInput>,
    ) -> CallToolResult {
        let id = TermId::from_uuid(input.id);
        let actor = caller.actor();
        // Named in the answer and the audit row; a failed read is the delete's own error.
        let term = match self.api.taxonomy.get_term(actor, id).await {
            Ok(t) => t,
            Err(e) => return results::from_core(e),
        };
        match self.api.taxonomy.delete_term(actor, id).await {
            Ok(()) => {
                self.audit(
                    &caller,
                    "term_delete",
                    "term",
                    Some(id.to_string()),
                    json!({"taxonomy": term.taxonomy, "name": term.name}),
                )
                .await;
                let summary = format!("Deleted the {} term {:?}.", term.taxonomy, term.name);
                results::ok(
                    json!({"id": id.as_uuid(), "taxonomy": term.taxonomy, "deleted": true}),
                    summary,
                )
            }
            Err(e) => results::from_core(e),
        }
    }
}

/// `y` or `ies` for "taxonom".
fn plural_y(n: usize) -> &'static str {
    if n == 1 { "y" } else { "ies" }
}
