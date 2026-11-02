//! Site tools (design 12 §5.2): `site_overview` and `site_search`.

use ferrite_core::{CoreError, SiteSearch, settings::site_title_or_configured};
use ferrite_domain::{ContentId, ContentItem, ContentStatus, MEDIA_KIND, Timestamp};
use rmcp::{handler::server::wrapper::Parameters, model::CallToolResult, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::per_page;
use crate::{
    api::problem::Problem,
    mcp::{Caller, FerriteMcp, results},
};

/// What `site_overview` returns (also the `ferrite://site` resource).
#[derive(Debug, Clone, Serialize)]
pub struct Overview {
    pub site: SiteInfo,
    pub caller: CallerInfo,
    /// `None` without `content:read` (see `unavailable`).
    pub content_types: Option<Vec<TypeInfo>>,
    pub taxonomies: Option<Vec<TaxonomyInfo>>,
    /// `None` without `menus:read`.
    pub menus: Option<Vec<MenuInfo>>,
    /// The sections left out, and the permission each needs.
    pub unavailable: Vec<Unavailable>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SiteInfo {
    pub title: String,
    pub url: String,
    pub admin_url: String,
    pub mcp_url: String,
    /// The FerriteCMS version.
    pub version: &'static str,
    /// The theme this node renders with.
    pub theme: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CallerInfo {
    pub login: String,
    pub display_name: String,
    pub role: String,
    /// The token's scopes (the effective rights are the role's within these).
    pub scopes: Vec<String>,
    /// `oauth` (a connected app) or `api_token` (a personal API token).
    pub connection: &'static str,
    /// The client's name as the audit log records it.
    pub client: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TypeInfo {
    pub name: String,
    pub label: String,
    pub hierarchical: bool,
    pub has_archive: bool,
    pub built_in: bool,
    /// Items per status, as the caller may read them.
    pub counts: Counts,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Counts {
    pub draft: u64,
    pub pending: u64,
    pub scheduled: u64,
    pub published: u64,
    pub private: u64,
    pub trash: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TaxonomyInfo {
    pub name: String,
    pub label: String,
    pub hierarchical: bool,
    pub content_types: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MenuInfo {
    pub name: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Unavailable {
    pub section: &'static str,
    pub reason: String,
}

/// The input of `site_search`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchInput {
    /// Words to look for in titles and bodies (plain text, no query syntax).
    pub q: String,
    /// Only these content types (names from site_overview); default: every type but
    /// media.
    #[serde(default)]
    pub types: Option<Vec<String>>,
    /// Results per page, 1 to 100 (default 20).
    #[serde(default)]
    #[schemars(range(min = 1, max = 100))]
    pub per_page: Option<u32>,
    /// `next_cursor` of the previous page, to get the next one.
    #[serde(default)]
    pub cursor: Option<String>,
}

/// One `site_search` hit: the fields of a content_list summary that a search knows.
#[derive(Debug, Clone, Serialize)]
pub struct SearchHit {
    pub id: ContentId,
    #[serde(rename = "type")]
    pub kind: String,
    pub title: String,
    pub status: String,
    pub version: i64,
    pub excerpt: String,
    /// The public page when the item has one now, else its preview link.
    pub url: Option<String>,
    /// Whether `url` is the public page.
    pub public: bool,
    /// The item in the admin.
    pub admin_url: String,
    pub updated_at: Timestamp,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchPage {
    pub items: Vec<SearchHit>,
    /// Pass as `cursor` for the next page; absent on the last one.
    pub next_cursor: Option<String>,
}

#[tool_router(router = site_tools, vis = "pub(crate)")]
impl FerriteMcp {
    /// Describes the site and you: its title, URL, admin URL, FerriteCMS version and
    /// active theme; its content types (with item counts per status), taxonomies and
    /// menus; and the role and permission scopes you act with, and whether you are
    /// connected through OAuth or a personal API token. Call this first. Sections your
    /// scopes do not cover are left out and listed under `unavailable`. For users,
    /// tokens, plugin installs and backups, send the user to the admin URL.
    #[tool(annotations(
        title = "Site overview",
        read_only_hint = true,
        open_world_hint = false
    ))]
    pub async fn site_overview(&self, caller: Caller) -> CallToolResult {
        match self.overview(&caller).await {
            Ok(o) => {
                let summary = format!(
                    "{} ({}): you are {} as {} via {}.",
                    o.site.title, o.site.url, o.caller.login, o.caller.role, o.caller.client
                );
                results::ok(o, summary)
            }
            Err(e) => results::from_core(e),
        }
    }

    /// Full-text search over the site's content (posts, pages and custom types; not
    /// media) that you may read: published items, and drafts and other unpublished
    /// items as your role allows. Returns the best matches first with id, type, title,
    /// status, `version`, excerpt, `url` (the public page, or a preview link while it has
    /// none; `public` says which) and `admin_url`, as content_list does; use content_get
    /// with the id for the whole item. Pages through results with `cursor`.
    #[tool(annotations(
        title = "Search content",
        read_only_hint = true,
        open_world_hint = false
    ))]
    pub async fn site_search(
        &self,
        caller: Caller,
        Parameters(input): Parameters<SearchInput>,
    ) -> CallToolResult {
        let limit = match per_page(input.per_page) {
            Ok(n) => n,
            Err(e) => return e,
        };
        let search = SiteSearch {
            q: input.q,
            kinds: input.types.unwrap_or_default(),
            limit,
            cursor: input.cursor,
        };
        match self.search(&caller, &search).await {
            Ok(page) => {
                let summary = match page.items.len() {
                    0 => format!("No content matches {:?}.", search.q.trim()),
                    n => format!(
                        "{n} result{} for {:?}{}.",
                        if n == 1 { "" } else { "s" },
                        search.q.trim(),
                        if page.next_cursor.is_some() {
                            "; more with next_cursor"
                        } else {
                            ""
                        }
                    ),
                };
                results::ok(page, summary)
            }
            Err(p) => results::from_problem(p),
        }
    }
}

impl FerriteMcp {
    /// The overview of `caller`'s view of the site.
    pub async fn overview(&self, caller: &Caller) -> Result<Overview, CoreError> {
        let api = &self.api;
        let actor = caller.actor();
        let mut unavailable = Vec::new();
        let title = site_title_or_configured(api.store.as_ref(), &api.site_title).await;
        let theme = api.site.theme.load().name().to_owned();
        let types = match section(
            "content_types",
            api.types.list(actor).await,
            &mut unavailable,
        )? {
            Some(types) => {
                let mut out = Vec::new();
                for t in types.into_iter().filter(|t| t.name != MEDIA_KIND) {
                    let c = api.content.counts(actor, &t.name, None).await?;
                    out.push(TypeInfo {
                        counts: Counts {
                            draft: c.draft,
                            pending: c.pending,
                            scheduled: c.scheduled,
                            published: c.published,
                            private: c.private,
                            trash: c.trash,
                        },
                        name: t.name,
                        label: t.label,
                        hierarchical: t.hierarchical,
                        has_archive: t.has_archive,
                        built_in: t.built_in,
                    });
                }
                Some(out)
            }
            None => None,
        };
        let taxonomies = section(
            "taxonomies",
            api.taxonomy.list_taxonomies(actor).await,
            &mut unavailable,
        )?
        .map(|list| {
            list.into_iter()
                .map(|t| TaxonomyInfo {
                    name: t.name,
                    label: t.label,
                    hierarchical: t.hierarchical,
                    content_types: t.content_types,
                })
                .collect()
        });
        let menus =
            section("menus", api.menus.list_for(actor).await, &mut unavailable)?.map(|list| {
                list.into_iter()
                    .map(|m| MenuInfo {
                        name: m.name,
                        label: m.label,
                    })
                    .collect()
            });
        let user = &caller.who.user;
        Ok(Overview {
            site: SiteInfo {
                title,
                url: api.site_url.clone(),
                admin_url: format!("{}/admin/", api.site_url),
                mcp_url: format!("{}{}", api.site_url, crate::mcp::PATH),
                version: env!("CARGO_PKG_VERSION"),
                theme,
            },
            caller: CallerInfo {
                login: user.login.clone(),
                display_name: user.display_name.clone(),
                role: user.role.as_str().to_owned(),
                scopes: actor.scopes.to_strings(),
                connection: if caller.via_oauth() {
                    "oauth"
                } else {
                    "api_token"
                },
                client: caller.client.clone(),
            },
            content_types: types,
            taxonomies,
            menus,
            unavailable,
        })
    }

    /// One page of `site_search`, with each hit's public URL.
    async fn search(&self, caller: &Caller, search: &SiteSearch) -> Result<SearchPage, Problem> {
        let actor = caller.actor();
        let page = self.api.content.search(actor, search).await?;
        let ids: Vec<ContentId> = page.items.iter().map(|i| i.id).collect();
        let public = self.public_urls(actor, &ids).await?;
        let items = page
            .items
            .into_iter()
            .map(|item: ContentItem| {
                let id = item.id.as_uuid();
                let (url, is_public) = self.url_of(
                    id,
                    item.status == ContentStatus::Trash,
                    public.get(&item.id),
                );
                SearchHit {
                    url,
                    public: is_public,
                    admin_url: self.admin_url(&item.kind, id),
                    id: item.id,
                    status: item.status.as_str().to_owned(),
                    version: item.version,
                    updated_at: item.updated_at,
                    kind: item.kind,
                    title: item.title,
                    excerpt: item.excerpt,
                }
            })
            .collect();
        Ok(SearchPage {
            items,
            next_cursor: page.next_cursor,
        })
    }
}

/// `Ok(Some)` for a readable section, `Ok(None)` (recorded in `unavailable`) when the
/// caller lacks the permission, else the error.
fn section<T>(
    name: &'static str,
    read: Result<T, CoreError>,
    unavailable: &mut Vec<Unavailable>,
) -> Result<Option<T>, CoreError> {
    match read {
        Ok(v) => Ok(Some(v)),
        Err(CoreError::Forbidden(what)) => {
            unavailable.push(Unavailable {
                section: name,
                reason: format!("needs {what}"),
            });
            Ok(None)
        }
        Err(e) => Err(e),
    }
}
