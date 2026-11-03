//! Content tools (design 12 §5.1 to §5.3): `content_list`, `content_get`,
//! `content_create`, `content_update`, `content_publish`, `content_unpublish`,
//! `content_schedule`, `content_submit_for_review`, `content_trash`, `content_restore`,
//! `content_revisions`, `content_restore_revision` and `content_preview_link`, and the
//! `ferrite://content/{type}/{id}` resource.
//!
//! The tools are adapters over the REST layer's pieces ([`crate::api::content`]: type and
//! item loading, the DTO conversion, the transitions, the list query) and core. Core
//! checks every permission; nothing here does.
//!
//! * Bodies are Markdown both ways: `body_markdown` is converted with the Markdown
//!   importer's converter, `body_blocks` is the editor's block JSON, and reads return
//!   `body_markdown` from the stored blocks. A leading `# H1` is the title when the call
//!   gives none, is dropped when it equals the given title, and is refused when it
//!   differs. Every `ferrite:media/<id>` image and the featured image must name media
//!   the caller can read.
//! * A write takes the `version` read last; a stale one is an error result whose
//!   `current` holds the item as stored now, shaped like a read, so the model can
//!   re-apply its change (the REST 412 rule).
//! * An item's `url` is its public page, or its preview link while it has none (trashed
//!   items have neither). `admin_url` is the editor screen.

use std::collections::{BTreeSet, HashMap};

use chrono::{DateTime, Utc};
use ferrite_core::{
    Actor, ContentDraft, CoreError, MAX_REVISIONS_LISTED,
    blocks::{self, to_markdown},
    content::{MAX_REFS, Robots, SeoPatch},
};
use ferrite_domain::{ContentId, ContentItem, MEDIA_KIND, RevisionId, TermId, Timestamp};
use ferrite_import_md::markdown_to_body;
use rmcp::{handler::server::wrapper::Parameters, model::CallToolResult, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use super::{PER_PAGE, per_page};
use crate::{
    api::{
        content::{
            ContentListParams, ListOrder, Transition, apply_transition, author_of, authors, dto,
            load, pick, served,
        },
        dto::content::{
            AllowedActionsDto, AuthorRefDto, ContentSummaryDto, RevisionSummaryDto, SeoDto,
            TranslationRefDto, nullable,
        },
        problem::Problem,
    },
    mcp::{
        Caller, FerriteMcp,
        results::{self, bad_input},
    },
};

/// The largest `body_markdown`, in bytes.
pub const MAX_BODY_MARKDOWN: usize = 1 << 20;

// ---- inputs ------------------------------------------------------------------------------

/// How `content_list` orders its items (ignored with `q`, which orders by relevance).
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Order {
    /// Newest publish time first; never published last.
    Published,
    /// Newest change first (the default).
    Updated,
    /// A to Z.
    Title,
    /// The editor's manual order (pages in a menu or tree).
    Menu,
}

impl From<Order> for ListOrder {
    fn from(o: Order) -> Self {
        match o {
            Order::Published => ListOrder::Published,
            Order::Updated => ListOrder::Updated,
            Order::Title => ListOrder::Title,
            Order::Menu => ListOrder::Menu,
        }
    }
}

/// The input of `content_list`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListInput {
    /// The content type: `post`, `page` or a custom type from site_overview.
    #[serde(rename = "type")]
    pub kind: String,
    /// Only these statuses, any of `draft`, `pending`, `scheduled`, `published`,
    /// `private`, `trash`. Default: every status but `trash`.
    #[serde(default)]
    pub status: Option<Vec<String>>,
    /// Words to look for in titles and bodies (plain text). Best matches first; cannot
    /// be combined with `author`, `term` or `parent`.
    #[serde(default)]
    pub q: Option<String>,
    /// Only items written by this user id.
    #[serde(default)]
    pub author: Option<Uuid>,
    /// Only items with this term id (a category or tag).
    #[serde(default)]
    pub term: Option<Uuid>,
    /// `root` for top-level items, or the id of a parent page.
    #[serde(default)]
    pub parent: Option<String>,
    /// Sort order; default `updated`.
    #[serde(default)]
    pub order: Option<Order>,
    /// Items per page, 1 to 100 (default 20).
    #[serde(default)]
    #[schemars(range(min = 1, max = 100))]
    pub per_page: Option<u32>,
    /// `next_cursor` of the previous page of the same query, to get the next page.
    #[serde(default)]
    pub cursor: Option<String>,
    /// Only items in this locale (a tag site_overview lists, e.g. `de`).
    #[serde(default)]
    pub locale: Option<String>,
}

/// The input of `content_translate`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TranslateInput {
    /// The item's content type (`post`, `page`, ...).
    #[serde(rename = "type")]
    pub kind: String,
    /// The item to translate.
    pub id: Uuid,
    /// The target locale (a tag site_overview lists, not the item's own).
    pub locale: String,
}

/// An item named by type and id (`content_get`, `content_preview_link`).
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ItemInput {
    /// The item's content type (`post`, `page`, ...).
    #[serde(rename = "type")]
    pub kind: String,
    /// The item's id.
    pub id: Uuid,
}

/// An item at the version the caller read (the lifecycle tools).
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VersionedInput {
    /// The item's content type (`post`, `page`, ...).
    #[serde(rename = "type")]
    pub kind: String,
    /// The item's id.
    pub id: Uuid,
    /// The `version` you last read. If someone changed the item since, the call fails
    /// and returns the current item so you can re-apply your change.
    #[schemars(range(min = 1))]
    pub version: i64,
}

/// The input of `content_schedule`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScheduleInput {
    /// The item's content type (`post`, `page`, ...).
    #[serde(rename = "type")]
    pub kind: String,
    /// The item's id.
    pub id: Uuid,
    /// The `version` you last read.
    #[schemars(range(min = 1))]
    pub version: i64,
    /// When to publish, as an RFC 3339 time with its zone, for example
    /// `2026-12-01T09:00:00Z`. Must be in the future.
    pub at: String,
}

/// The input of `content_revisions`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RevisionsInput {
    /// The item's content type (`post`, `page`, ...).
    #[serde(rename = "type")]
    pub kind: String,
    /// The item's id.
    pub id: Uuid,
    /// A revision id from the list: returns that revision in full (title, excerpt,
    /// `body_markdown`, fields) instead of the list.
    #[serde(default)]
    pub revision: Option<Uuid>,
    /// Revisions to list, 1 to 50 (default 20; the newest are kept).
    #[serde(default)]
    #[schemars(range(min = 1, max = 50))]
    pub per_page: Option<u32>,
}

/// The input of `content_restore_revision`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RestoreRevisionInput {
    /// The item's content type (`post`, `page`, ...).
    #[serde(rename = "type")]
    pub kind: String,
    /// The item's id.
    pub id: Uuid,
    /// The revision to bring back (from content_revisions).
    pub revision: Uuid,
    /// The item's current `version`.
    #[schemars(range(min = 1))]
    pub version: i64,
}

/// A robots directive of an item's page.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum RobotsInput {
    NoArchive,
    NoFollow,
    NoImageIndex,
    NoIndex,
    NoSnippet,
}

impl From<RobotsInput> for Robots {
    fn from(r: RobotsInput) -> Self {
        match r {
            RobotsInput::NoArchive => Self::NoArchive,
            RobotsInput::NoFollow => Self::NoFollow,
            RobotsInput::NoImageIndex => Self::NoImageIndex,
            RobotsInput::NoIndex => Self::NoIndex,
            RobotsInput::NoSnippet => Self::NoSnippet,
        }
    }
}

/// A change to an item's SEO meta. Per member: leave it out to keep it, send `null` or
/// a blank string to delete it, any other value sets it.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SeoInput {
    /// The page's whole `<title>` (at most 200 characters, no site suffix is added).
    #[serde(default, deserialize_with = "nullable")]
    #[schemars(with = "Option<String>")]
    pub title: Option<Option<String>>,
    /// The meta description (at most 500 characters; search results show about 160).
    #[serde(default, deserialize_with = "nullable")]
    #[schemars(with = "Option<String>")]
    pub description: Option<Option<String>>,
    /// An absolute http(s) URL for the canonical link.
    #[serde(default, deserialize_with = "nullable")]
    #[schemars(with = "Option<String>")]
    pub canonical: Option<Option<String>>,
    /// Robots directives; `[]` or `null` removes them.
    #[serde(default, deserialize_with = "nullable")]
    #[schemars(with = "Option<Vec<RobotsInput>>")]
    pub robots: Option<Option<Vec<RobotsInput>>>,
}

impl From<SeoInput> for SeoPatch {
    fn from(s: SeoInput) -> Self {
        Self {
            title: s.title,
            description: s.description,
            canonical: s.canonical,
            robots: s
                .robots
                .map(|r| r.map(|v| v.into_iter().map(Into::into).collect())),
        }
    }
}

/// The input of `content_create`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateInput {
    /// The content type: `post`, `page` or a custom type from site_overview.
    #[serde(rename = "type")]
    pub kind: String,
    /// The title. Optional only when `body_markdown` starts with a `# Heading`, which
    /// then becomes the title.
    #[serde(default)]
    pub title: Option<String>,
    /// The URL slug. Generated from the title when absent; a taken slug gets `-2`,
    /// `-3`, ...
    #[serde(default)]
    pub slug: Option<String>,
    /// A short summary. Derived from the body (160 characters) when absent.
    #[serde(default)]
    pub excerpt: Option<String>,
    /// The body as Markdown (CommonMark plus tables, at most 1 MiB). Start with
    /// paragraphs or `##` headings, not `# H1`: the title is separate. Images are
    /// `![alt](ferrite:media/<id>)` with a media id from media_list or media_upload.
    /// Embeds, raw HTML and anything else Markdown cannot say go in a fenced
    /// ```ferrite-block JSON block, exactly as content_get returns them. Cannot be sent
    /// together with body_blocks.
    #[serde(default)]
    pub body_markdown: Option<String>,
    /// The body as the editor's block JSON (`{"type":"doc","content":[...]}`), for exact
    /// control. Cannot be sent together with body_markdown.
    #[serde(default)]
    pub body_blocks: Option<Value>,
    /// Custom field values as an object, as the content type defines them. The
    /// featured image can also be sent as `featured_image` inside it.
    #[serde(default)]
    pub fields: Option<Map<String, Value>>,
    /// Term ids (categories, tags) in order; find them with terms_list.
    #[serde(default)]
    pub terms: Option<Vec<Uuid>>,
    /// The parent page's id (hierarchical types such as `page` only).
    #[serde(default)]
    pub parent: Option<Uuid>,
    /// The featured image: the id of a media item.
    #[serde(default)]
    pub featured_image: Option<Uuid>,
    /// SEO title, description, canonical URL and robots directives.
    #[serde(default)]
    pub seo: Option<SeoInput>,
    /// The item's locale (a tag site_overview lists); default the site's default.
    #[serde(default)]
    pub locale: Option<String>,
    /// An item of the same type this one translates (joins its translation group).
    #[serde(default)]
    pub translation_of: Option<Uuid>,
}

/// The input of `content_update`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateInput {
    /// The item's content type (`post`, `page`, ...).
    #[serde(rename = "type")]
    pub kind: String,
    /// The item's id.
    pub id: Uuid,
    /// The `version` you last read. If someone changed the item since, nothing is
    /// written and the current item is returned so you can re-apply your change.
    #[schemars(range(min = 1))]
    pub version: i64,
    /// A new title. Leave out to keep it (a leading `# Heading` in body_markdown renames
    /// the item when no title is sent).
    #[serde(default)]
    pub title: Option<String>,
    /// A new URL slug (changing a published item's slug adds a redirect from the old
    /// address).
    #[serde(default)]
    pub slug: Option<String>,
    /// A new excerpt; `null` derives it from the body again.
    #[serde(default, deserialize_with = "nullable")]
    #[schemars(with = "Option<String>")]
    pub excerpt: Option<Option<String>>,
    /// The new body as Markdown, replacing the whole body (CommonMark plus tables, at
    /// most 1 MiB). Send the complete text, not a patch. Do not start with `# H1`: the
    /// title is separate. Images are `![alt](ferrite:media/<id>)`; embeds, raw HTML and
    /// anything else Markdown cannot say stay in their ```ferrite-block fences as
    /// content_get returns them. Cannot be sent together with body_blocks.
    #[serde(default)]
    pub body_markdown: Option<String>,
    /// The new body as the editor's block JSON, replacing the whole body. Cannot be sent
    /// together with body_markdown.
    #[serde(default)]
    pub body_blocks: Option<Value>,
    /// Custom field values: the members you send replace the same members, the others
    /// are kept, and a member sent as `null` is cleared (removed).
    #[serde(default)]
    pub fields: Option<Map<String, Value>>,
    /// Replaces the item's terms with these ids (`[]` removes them all); leave out to
    /// keep them.
    #[serde(default)]
    pub terms: Option<Vec<Uuid>>,
    /// The parent page's id; `null` moves the item to the top level.
    #[serde(default, deserialize_with = "nullable")]
    #[schemars(with = "Option<Uuid>")]
    pub parent: Option<Option<Uuid>>,
    /// The featured image (a media id); `null` clears it.
    #[serde(default, deserialize_with = "nullable")]
    #[schemars(with = "Option<Uuid>")]
    pub featured_image: Option<Option<Uuid>>,
    /// Moves the item to another locale (a published item's old address redirects).
    #[serde(default)]
    pub locale: Option<String>,
    /// Joins the translation group of this item; `null` leaves the item's group.
    #[serde(default, deserialize_with = "nullable")]
    #[schemars(with = "Option<Uuid>")]
    pub translation_of: Option<Option<Uuid>>,
    /// SEO changes, member by member.
    #[serde(default)]
    pub seo: Option<SeoInput>,
}

// ---- outputs -----------------------------------------------------------------------------

/// An item as the tools return it.
#[derive(Debug, Serialize)]
pub struct ItemOut {
    pub id: Uuid,
    #[serde(rename = "type")]
    pub kind: String,
    pub title: String,
    pub slug: String,
    /// `draft`, `pending`, `scheduled`, `published`, `private` or `trash`.
    pub status: String,
    /// Send this back with the next write.
    pub version: i64,
    pub excerpt: String,
    pub body_markdown: String,
    pub fields: Value,
    /// Term ids in order.
    pub terms: Vec<Uuid>,
    pub parent: Option<Uuid>,
    /// The media id of the featured image.
    pub featured_image: Option<String>,
    pub seo: SeoDto,
    pub author: AuthorRefDto,
    /// What you may do with the item now.
    pub allowed: AllowedActionsDto,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub published_at: Option<DateTime<Utc>>,
    pub scheduled_at: Option<DateTime<Utc>>,
    /// The public page, or the preview link while there is none; `null` when trashed.
    pub url: Option<String>,
    /// Whether `url` is the live public page (otherwise it is a preview link, which only
    /// works in a browser signed in to the site).
    pub public: bool,
    /// The item in the admin editor.
    pub admin_url: String,
    /// What the Markdown converter changed or dropped, one sentence each.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    /// The item's locale.
    pub locale: String,
    /// The item's other translations you may read: id, locale, status, title, permalink.
    pub translations: Vec<TranslationRefDto>,
}

/// An item in a list: no body.
#[derive(Debug, Serialize)]
pub struct SummaryOut {
    pub id: Uuid,
    #[serde(rename = "type")]
    pub kind: String,
    pub title: String,
    pub slug: String,
    pub status: String,
    pub version: i64,
    pub excerpt: String,
    pub parent: Option<Uuid>,
    pub author: AuthorRefDto,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub published_at: Option<DateTime<Utc>>,
    pub scheduled_at: Option<DateTime<Utc>>,
    pub url: Option<String>,
    pub public: bool,
    pub admin_url: String,
    pub locale: String,
}

/// One page of `content_list`.
#[derive(Debug, Serialize)]
pub struct ListOut {
    pub items: Vec<SummaryOut>,
    /// Pass as `cursor` for the next page; `null` on the last one.
    pub next_cursor: Option<String>,
}

/// One revision in full.
#[derive(Debug, Serialize)]
pub struct RevisionOut {
    pub id: Uuid,
    pub title: String,
    pub excerpt: String,
    pub body_markdown: String,
    pub fields: Value,
    pub note: Option<String>,
    pub author: AuthorRefDto,
    pub created_at: DateTime<Utc>,
}

/// The revisions of an item, newest first.
#[derive(Debug, Serialize)]
pub struct RevisionsOut {
    /// The item's current version, for content_restore_revision.
    pub item_version: i64,
    pub items: Vec<RevisionSummaryDto>,
}

/// What `content_preview_link` returns.
#[derive(Debug, Serialize)]
pub struct PreviewOut {
    pub url: String,
    /// The link does not expire: it works for as long as the item exists and the
    /// signed-in viewer may read it.
    pub expires_at: Option<DateTime<Utc>>,
    /// The preview is shown only to a browser signed in to the site; opening it with
    /// the API token fails.
    pub requires_sign_in: bool,
    pub note: String,
}

// ---- tools -------------------------------------------------------------------------------

#[tool_router(router = content_tools, vis = "pub(crate)")]
impl FerriteMcp {
    /// Lists items of one content type as summaries (no bodies), by default the newest
    /// changes first and without trashed items. Filter by status, by search words (`q`,
    /// best matches first), author, term or parent; page with `cursor`. Each item has
    /// id, title, slug, status, version, excerpt, author, dates, `url` and `admin_url`.
    /// Call content_get with the id for the whole item.
    #[tool(annotations(title = "List content", read_only_hint = true, open_world_hint = false))]
    pub async fn content_list(
        &self,
        caller: Caller,
        Parameters(input): Parameters<ListInput>,
    ) -> CallToolResult {
        let limit = match per_page(input.per_page) {
            Ok(n) => n,
            Err(e) => return e,
        };
        match self.list_items(&caller, input, limit).await {
            Ok((out, kind)) => {
                let n = out.items.len();
                let noun = format!("{kind} item");
                let summary = format!(
                    "{}.",
                    results::page_summary(n, &noun, out.next_cursor.is_some())
                );
                results::ok(out, summary)
            }
            Err(p) => results::from_problem(p),
        }
    }

    /// Returns one item whole: title, slug, status, `version` (needed for every write),
    /// excerpt, `body_markdown`, custom fields, term ids, parent, featured image, SEO,
    /// author, dates, the public or preview `url`, and what you may do with it now
    /// (`allowed`). Embeds and other blocks Markdown cannot express appear as
    /// ```ferrite-block fences: keep them as they are when you edit the text.
    #[tool(annotations(title = "Get content", read_only_hint = true, open_world_hint = false))]
    pub async fn content_get(
        &self,
        caller: Caller,
        Parameters(input): Parameters<ItemInput>,
    ) -> CallToolResult {
        let run = async {
            let actor = caller.actor();
            let item = load(&self.api, actor, &input.kind, input.id).await?;
            self.item_out(actor, item, Vec::new()).await
        };
        match run.await {
            Ok(out) => {
                let summary = format!(
                    "{:?} ({} {}, version {}).",
                    out.title, out.status, out.kind, out.version
                );
                results::ok(out, summary)
            }
            Err(p) => results::from_problem(p),
        }
    }

    /// Creates a new item as a draft, written by you; nothing is public until
    /// content_publish. Send `title` and a Markdown `body_markdown` (CommonMark plus
    /// tables). Do not start the body with a `# H1`: the title is separate (a leading H1
    /// becomes the title when you send none, and is dropped when it equals it).
    /// Images are `![alt](ferrite:media/<id>)` using ids of media you can read; embeds,
    /// raw HTML and other blocks are ```ferrite-block fences holding the block's JSON.
    /// Read ferrite://guide/writing first for the site's conventions. Returns the new
    /// item with its `id`, `version`, preview `url` and `admin_url`, plus `notes` when
    /// the converter dropped or changed something.
    #[tool(annotations(
        title = "Create content",
        read_only_hint = false,
        destructive_hint = false,
        idempotent_hint = false,
        open_world_hint = false
    ))]
    pub async fn content_create(
        &self,
        caller: Caller,
        Parameters(input): Parameters<CreateInput>,
    ) -> CallToolResult {
        match self.create_item(&caller, input).await {
            Ok(out) => {
                let summary = with_notes(
                    format!(
                        "Created the draft {:?} ({}, id {}, version {}).",
                        out.title, out.kind, out.id, out.version
                    ),
                    &out.notes,
                );
                results::ok(out.value, summary)
            }
            Err(p) => results::from_problem(p),
        }
    }

    /// Copies an item as a new draft in another locale (the site's locales are in
    /// site_overview) and returns it with `body_markdown`: translate the title, body and
    /// excerpt yourself, then save them with content_update (send its `version`). The
    /// draft keeps the source's slug, fields and SEO meta; its parent and terms are their
    /// translations, and `notes` says which had none. Fails when the item already has a
    /// translation in that locale (the error names it).
    #[tool(annotations(
        title = "Translate content",
        read_only_hint = false,
        destructive_hint = false,
        idempotent_hint = false,
        open_world_hint = false
    ))]
    pub async fn content_translate(
        &self,
        caller: Caller,
        Parameters(input): Parameters<TranslateInput>,
    ) -> CallToolResult {
        let api = &self.api;
        let actor = caller.actor();
        let done = async {
            let source = crate::api::content::load(api, actor, &input.kind, input.id).await?;
            let t = api
                .content
                .translate(actor, source.id, &input.locale)
                .await?;
            let notes: Vec<String> = t
                .notes
                .iter()
                .map(ferrite_core::TranslateNote::code)
                .collect();
            self.audit(
                &caller,
                "content_translate",
                "content",
                Some(t.item.id.to_string()),
                json!({"type": t.item.kind, "source": input.id, "locale": input.locale}),
            )
            .await;
            let out = self.item_out(actor, t.item, notes).await?;
            Ok::<_, Problem>(out)
        }
        .await;
        match done {
            Ok(out) => {
                let summary = with_notes(
                    format!(
                        "Created the {} draft {:?} (id {}, version {}); translate it, then save it with content_update.",
                        out.locale, out.title, out.id, out.version
                    ),
                    &out.notes,
                );
                match serde_json::to_value(&out) {
                    Ok(v) => results::ok(v, summary),
                    Err(_) => results::from_problem(Problem::internal()),
                }
            }
            Err(p) => results::from_problem(p),
        }
    }

    /// Changes an item; what you leave out is kept. Send the `version` from your last
    /// read: if someone saved since, nothing is written and the error carries the
    /// current item (`current`), so re-apply your change to it. `body_markdown` or
    /// `body_blocks` replaces the whole body, so send the complete text, with the
    /// ```ferrite-block fences of content_get untouched. Do not start the Markdown with
    /// a `# H1`: it would rename the item (when you send no title) or be refused (when
    /// it differs from `title`). Images are `![alt](ferrite:media/<id>)`. See
    /// ferrite://guide/writing. Editing a published item changes the live page at once;
    /// status changes have their own tools. Returns the updated item.
    #[tool(annotations(
        title = "Update content",
        read_only_hint = false,
        destructive_hint = false,
        idempotent_hint = true,
        open_world_hint = false
    ))]
    pub async fn content_update(
        &self,
        caller: Caller,
        Parameters(input): Parameters<UpdateInput>,
    ) -> CallToolResult {
        match self.update_item(&caller, input).await {
            Ok(out) => {
                let summary = with_notes(
                    format!(
                        "Updated {:?} ({}, now version {}).",
                        out.title, out.status, out.version
                    ),
                    &out.notes,
                );
                results::ok(out.value, summary)
            }
            Err(p) => results::from_problem(p),
        }
    }

    /// Publishes a draft, pending or scheduled item now: it goes live on the public
    /// site at once. Needs the right to publish (contributors cannot: they use
    /// content_submit_for_review). Ask the user before publishing unless they asked for
    /// it. Send the `version` you last read. Returns the item with its public `url`.
    #[tool(annotations(
        title = "Publish content",
        read_only_hint = false,
        destructive_hint = false,
        idempotent_hint = true,
        open_world_hint = false
    ))]
    pub async fn content_publish(
        &self,
        caller: Caller,
        Parameters(input): Parameters<VersionedInput>,
    ) -> CallToolResult {
        self.run_transition(
            &caller,
            input,
            Transition::Publish,
            "content_publish",
            "Published",
        )
        .await
    }

    /// Takes a published, scheduled or private item off the public site and turns it
    /// back into a draft. Its address stops working until it is published again. Ask the
    /// user first. Send the `version` you last read.
    #[tool(annotations(
        title = "Unpublish content",
        read_only_hint = false,
        destructive_hint = true,
        idempotent_hint = true,
        open_world_hint = false
    ))]
    pub async fn content_unpublish(
        &self,
        caller: Caller,
        Parameters(input): Parameters<VersionedInput>,
    ) -> CallToolResult {
        self.run_transition(
            &caller,
            input,
            Transition::Unpublish,
            "content_unpublish",
            "Unpublished",
        )
        .await
    }

    /// Offers a draft for review: it becomes `pending` and waits for someone who may
    /// publish. This is how contributors finish their work. Send the `version` you last
    /// read.
    #[tool(annotations(
        title = "Submit content for review",
        read_only_hint = false,
        destructive_hint = false,
        idempotent_hint = true,
        open_world_hint = false
    ))]
    pub async fn content_submit_for_review(
        &self,
        caller: Caller,
        Parameters(input): Parameters<VersionedInput>,
    ) -> CallToolResult {
        self.run_transition(
            &caller,
            input,
            Transition::Submit,
            "content_submit_for_review",
            "Submitted for review",
        )
        .await
    }

    /// Schedules a draft or pending item to be published automatically at `at` (an RFC
    /// 3339 time in the future, with its zone). It stays unpublished until then. Needs
    /// the right to publish. Send the `version` you last read.
    #[tool(annotations(
        title = "Schedule content",
        read_only_hint = false,
        destructive_hint = false,
        idempotent_hint = true,
        open_world_hint = false
    ))]
    pub async fn content_schedule(
        &self,
        caller: Caller,
        Parameters(input): Parameters<ScheduleInput>,
    ) -> CallToolResult {
        match self.schedule_item(&caller, input).await {
            Ok(out) => {
                let when = out.scheduled_at.map(|t| t.to_rfc3339()).unwrap_or_default();
                let summary = format!("Scheduled {:?} for {when}.", out.title);
                results::ok(out.value, summary)
            }
            Err(p) => results::from_problem(p),
        }
    }

    /// Moves an item to the trash; if it was public, its page disappears. It can be
    /// brought back with content_restore, so nothing is lost, but ask the user first.
    /// Send the `version` you last read.
    #[tool(annotations(
        title = "Trash content",
        read_only_hint = false,
        destructive_hint = true,
        idempotent_hint = true,
        open_world_hint = false
    ))]
    pub async fn content_trash(
        &self,
        caller: Caller,
        Parameters(input): Parameters<VersionedInput>,
    ) -> CallToolResult {
        self.trash_item(&caller, input).await
    }

    /// Brings a trashed item back as a draft (it is not republished). Send the
    /// `version` you last read.
    #[tool(annotations(
        title = "Restore content from the trash",
        read_only_hint = false,
        destructive_hint = false,
        idempotent_hint = true,
        open_world_hint = false
    ))]
    pub async fn content_restore(
        &self,
        caller: Caller,
        Parameters(input): Parameters<VersionedInput>,
    ) -> CallToolResult {
        self.run_transition(
            &caller,
            input,
            Transition::Restore,
            "content_restore",
            "Restored",
        )
        .await
    }

    /// Lists an item's saved revisions, newest first, each with its author, time, title
    /// and a note (such as "restored from ..."), plus the item's current `version`. Pass
    /// `revision` to get one revision in full with its `body_markdown`, to compare it
    /// with the current text. Only people who can edit the item see its revisions.
    #[tool(annotations(
        title = "Content revisions",
        read_only_hint = true,
        open_world_hint = false
    ))]
    pub async fn content_revisions(
        &self,
        caller: Caller,
        Parameters(input): Parameters<RevisionsInput>,
    ) -> CallToolResult {
        // Its own range: at most the 50 revisions kept, not the usual 100.
        let limit = match input.per_page {
            None => PER_PAGE.min(MAX_REVISIONS_LISTED),
            Some(n) if (1..=MAX_REVISIONS_LISTED).contains(&n) => n,
            Some(_) => {
                return results::invalid(
                    Some("per_page"),
                    format!("per_page must be 1..={MAX_REVISIONS_LISTED}"),
                );
            }
        };
        match self.revisions(&caller, input, limit).await {
            Ok(Either::List(out)) => {
                let summary = format!("{} revisions, newest first.", out.items.len());
                results::ok(out, summary)
            }
            Ok(Either::One(out)) => {
                let summary = format!("Revision {} of {:?}.", out.id, out.title);
                results::ok(out, summary)
            }
            Err(p) => results::from_problem(p),
        }
    }

    /// Brings an earlier revision's title, excerpt, body and fields back as a new
    /// version of the item (the history is kept, and the slug, parent and terms stay as
    /// they are). Send the item's current `version`. Returns the updated item.
    #[tool(annotations(
        title = "Restore a revision",
        read_only_hint = false,
        destructive_hint = false,
        idempotent_hint = true,
        open_world_hint = false
    ))]
    pub async fn content_restore_revision(
        &self,
        caller: Caller,
        Parameters(input): Parameters<RestoreRevisionInput>,
    ) -> CallToolResult {
        match self.restore_revision(&caller, input).await {
            Ok(out) => {
                let summary = format!(
                    "Restored a revision of {:?} (now version {}).",
                    out.title, out.version
                );
                results::ok(out.value, summary)
            }
            Err(p) => results::from_problem(p),
        }
    }

    /// Returns the link at which the item shows as it will look, drafts and scheduled
    /// items included. Give it to the user: the preview opens only in a browser where
    /// they are signed in to the site (it does not work with the API token), and it
    /// does not expire. Not available for trashed items.
    #[tool(annotations(title = "Preview link", read_only_hint = true, open_world_hint = false))]
    pub async fn content_preview_link(
        &self,
        caller: Caller,
        Parameters(input): Parameters<ItemInput>,
    ) -> CallToolResult {
        match self.preview_link(&caller, input).await {
            Ok(out) => {
                let summary = format!("Preview: {}", out.url);
                results::ok(out, summary)
            }
            Err(p) => results::from_problem(p),
        }
    }
}

enum Either {
    List(RevisionsOut),
    One(RevisionOut),
}

/// The result of a write that is done: the full item, or (when building it fails) a
/// minimal record, never an error (the model would retry a committed write).
struct Written {
    value: Value,
    id: Uuid,
    kind: String,
    title: String,
    status: String,
    version: i64,
    scheduled_at: Option<DateTime<Utc>>,
    notes: Vec<String>,
}

// ---- the work ----------------------------------------------------------------------------

/// `summary` with what the Markdown converter changed, if anything.
fn with_notes(mut summary: String, notes: &[String]) -> String {
    if !notes.is_empty() {
        summary.push_str("\nNotes from the Markdown conversion: ");
        summary.push_str(&notes.join("; "));
    }
    summary
}

/// A body converted for a write.
struct Body {
    /// The new body; `None` when the call sends none.
    body: Option<Value>,
    /// The argument it came from (`body_markdown` or `body_blocks`), for errors.
    field: &'static str,
    /// The title lifted from a leading `# H1` (only when the call gave no title).
    title: Option<String>,
    notes: Vec<String>,
}

/// Converts the body argument of a write (see the module docs for the H1 rule).
fn resolve_body(
    markdown: Option<String>,
    blocks: Option<Value>,
    title: Option<&str>,
) -> Result<Body, Problem> {
    match (markdown, blocks) {
        (Some(_), Some(_)) => Err(bad_input(
            Some("body_markdown"),
            "send body_markdown or body_blocks, not both",
        )),
        (None, None) => Ok(Body {
            body: None,
            field: "body_markdown",
            title: None,
            notes: Vec::new(),
        }),
        (None, Some(doc)) => {
            if !blocks::is_document(&doc) {
                return Err(bad_input(
                    Some("body_blocks"),
                    "body_blocks must be a document: {\"type\":\"doc\",\"content\":[...]}",
                ));
            }
            results::within_json_cap("body_blocks", &doc)?;
            Ok(Body {
                body: Some(doc),
                field: "body_blocks",
                title: None,
                notes: Vec::new(),
            })
        }
        (Some(markdown), None) => {
            if markdown.len() > MAX_BODY_MARKDOWN {
                return Err(bad_input(
                    Some("body_markdown"),
                    format!(
                        "body_markdown is {} bytes; the limit is {MAX_BODY_MARKDOWN} (1 MiB). \
                         Split the text over several items or shorten it",
                        markdown.len()
                    ),
                ));
            }
            let converted = markdown_to_body(&markdown);
            let given = title.map(str::trim).filter(|t| !t.is_empty());
            let lifted = match (given, converted.title) {
                (Some(g), Some(h)) if g.to_lowercase() == h.trim().to_lowercase() => None,
                (Some(g), Some(h)) => {
                    return Err(bad_input(
                        Some("body_markdown"),
                        format!(
                            "body_markdown starts with the heading \"# {h}\", which differs from \
                             the title \"{g}\". Put that text in `title`, or turn the heading \
                             into \"## {h}\" (the body does not repeat the title)"
                        ),
                    ));
                }
                (None, Some(h)) => Some(h),
                (_, None) => None,
            };
            Ok(Body {
                body: Some(converted.body),
                field: "body_markdown",
                title: lifted,
                notes: converted.notes,
            })
        }
    }
}

/// How deep [`media_in_body`] looks: objects and arrays both count a level, and a block
/// tree nests an object and an array per block level, so this covers the renderer's
/// [`blocks::MAX_DEPTH`] block levels.
const MEDIA_WALK_DEPTH: usize = 2 * blocks::MAX_DEPTH + 1;

/// The media ids that `body`'s image blocks name.
fn media_in_body(v: &Value, out: &mut BTreeSet<Uuid>, depth: usize) {
    if depth > MEDIA_WALK_DEPTH {
        return;
    }
    match v {
        Value::Object(map) => {
            if map.get("type").and_then(Value::as_str) == Some("image")
                && let Some(id) = map
                    .get("attrs")
                    .and_then(|a| a.get("src"))
                    .and_then(Value::as_str)
                    .and_then(|s| s.trim().strip_prefix("ferrite:media/"))
                    .and_then(|s| Uuid::parse_str(s).ok())
            {
                out.insert(id);
            }
            for v in map.values() {
                media_in_body(v, out, depth + 1);
            }
        }
        Value::Array(items) => {
            for v in items {
                media_in_body(v, out, depth + 1);
            }
        }
        _ => {}
    }
}

/// The `type` argument must be a registered content type other than media, else `404`.
async fn known_type(api: &crate::api::ApiState, actor: &Actor, kind: &str) -> Result<(), Problem> {
    served(kind)?;
    api.types.get(actor, kind).await?;
    Ok(())
}

impl FerriteMcp {
    /// The public paths of `ids` (those with a live page), prefixed with the site URL.
    pub(crate) async fn public_urls(
        &self,
        actor: &Actor,
        ids: &[ContentId],
    ) -> Result<HashMap<ContentId, String>, Problem> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let mut urls = HashMap::new();
        for chunk in ids.chunks(MAX_REFS) {
            for r in self.api.content.refs(actor, chunk).await? {
                if let Some(path) = r.permalink {
                    urls.insert(r.id, format!("{}{path}", self.api.site_url));
                }
            }
        }
        Ok(urls)
    }

    /// `(url, public)` of an item: its page, else its preview link (none when trashed).
    pub(crate) fn url_of(
        &self,
        id: Uuid,
        trashed: bool,
        public: Option<&String>,
    ) -> (Option<String>, bool) {
        match public {
            Some(url) => (Some(url.clone()), true),
            None if trashed => (None, false),
            None => (Some(format!("{}/_preview/{id}", self.api.site_url)), false),
        }
    }

    pub(crate) fn admin_url(&self, kind: &str, id: Uuid) -> String {
        format!("{}/admin/content/{kind}/{id}", self.api.site_url)
    }

    /// `item` as the tools return it.
    pub(crate) async fn item_out(
        &self,
        actor: &Actor,
        item: ContentItem,
        notes: Vec<String>,
    ) -> Result<ItemOut, Problem> {
        let d = dto(&self.api, actor, item).await?;
        let id = ContentId::from_uuid(d.id);
        let public = self.public_urls(actor, &[id]).await?;
        let (url, is_public) = self.url_of(d.id, d.status == "trash", public.get(&id));
        let featured_image = d
            .fields
            .get("featured_image")
            .and_then(Value::as_str)
            .map(str::to_owned);
        Ok(ItemOut {
            admin_url: self.admin_url(&d.kind, d.id),
            body_markdown: to_markdown(&d.body),
            id: d.id,
            kind: d.kind,
            title: d.title,
            slug: d.slug,
            status: d.status,
            version: d.version,
            excerpt: d.excerpt,
            fields: d.fields,
            terms: d.terms,
            parent: d.parent_id,
            featured_image,
            seo: d.seo,
            author: d.author,
            allowed: d.allowed,
            created_at: d.created_at,
            updated_at: d.updated_at,
            published_at: d.published_at,
            scheduled_at: d.scheduled_at,
            url,
            public: is_public,
            notes,
            locale: d.locale,
            translations: d.translations,
        })
    }

    /// The answer of a write that is committed: the item as the tools return it, or, if
    /// that cannot be built, a minimal record (id, version, status, admin URL) with a
    /// pointer to content_get. Never an error.
    async fn written(&self, actor: &Actor, item: ContentItem, notes: Vec<String>) -> Written {
        let (id, kind, title, status, version) = (
            item.id.as_uuid(),
            item.kind.clone(),
            item.title.clone(),
            item.status.as_str().to_owned(),
            item.version,
        );
        let scheduled_at = item.scheduled_at.map(|t| t.as_datetime());
        let admin_url = self.admin_url(&kind, id);
        let full = match self.item_out(actor, item, notes.clone()).await {
            Ok(out) => serde_json::to_value(out).map_err(|e| e.to_string()),
            Err(p) => Err(p.document().detail),
        };
        let value = full.unwrap_or_else(|reason| {
            tracing::warn!(%reason, item = %id, "item not built after a done write");
            let mut v = json!({
                "id": id, "type": kind, "title": title, "status": status,
                "version": version, "admin_url": admin_url,
                "note": "The change was saved, but the full item could not be built; \
                         call content_get for it.",
            });
            if !notes.is_empty() {
                v["notes"] = json!(notes);
            }
            v
        });
        Written {
            value,
            id,
            kind,
            title,
            status,
            version,
            scheduled_at,
            notes,
        }
    }

    /// The problem of a failed write to item `id`: a stale version carries the item as
    /// stored now (shaped like a read) in `current`.
    async fn write_problem(&self, actor: &Actor, id: ContentId, e: CoreError) -> Problem {
        let conflict = matches!(e, CoreError::Conflict { .. });
        let problem = Problem::from(e);
        if !conflict {
            return problem;
        }
        let current = async {
            let item = self.api.content.get_for(actor, id).await?;
            let out = self.item_out(actor, item, Vec::new()).await?;
            serde_json::to_value(out).map_err(|_| Problem::internal())
        }
        .await;
        match current {
            Ok(value) => problem.with_current(value),
            // Gone or unreadable since: the error stays without `current`.
            Err(_) => problem,
        }
    }

    /// Checks that every media id in `ids` names a media item `actor` can read (and that
    /// is not in the trash).
    async fn check_media(
        &self,
        actor: &Actor,
        field: &str,
        ids: &BTreeSet<Uuid>,
    ) -> Result<(), Problem> {
        if ids.is_empty() {
            return Ok(());
        }
        let wanted: Vec<ContentId> = ids.iter().copied().map(ContentId::from_uuid).collect();
        let mut found = BTreeSet::new();
        for chunk in wanted.chunks(MAX_REFS) {
            for r in self.api.content.refs(actor, chunk).await? {
                if r.kind == MEDIA_KIND && r.status != ferrite_domain::ContentStatus::Trash {
                    found.insert(r.id.as_uuid());
                }
            }
        }
        let missing: Vec<String> = ids
            .iter()
            .filter(|id| !found.contains(id))
            .map(Uuid::to_string)
            .collect();
        if missing.is_empty() {
            return Ok(());
        }
        Err(bad_input(
            Some(field),
            format!(
                "no media you can read has the id{} {}. Find media ids with media_list or \
                 upload the file with media_upload",
                if missing.len() == 1 { "" } else { "s" },
                missing.join(", ")
            ),
        ))
    }

    /// `check_media` for the images of a body (named by `field` in the error) and for
    /// the final `fields.featured_image`, however it was sent (the argument or inside
    /// `fields`), when it is a new value: `stored` is the value the item already has,
    /// which is not checked again.
    async fn check_body_media(
        &self,
        actor: &Actor,
        body: Option<(&Value, &str)>,
        fields: Option<&Map<String, Value>>,
        stored: Option<&Value>,
    ) -> Result<(), Problem> {
        if let Some((body, field)) = body {
            let mut ids = BTreeSet::new();
            media_in_body(body, &mut ids, 0);
            self.check_media(actor, field, &ids).await?;
        }
        let featured = fields.and_then(|f| f.get("featured_image"));
        if let Some(Value::String(s)) = featured
            && featured != stored
            // A malformed id is core's to refuse (it checks the shape).
            && let Ok(id) = Uuid::parse_str(s)
        {
            self.check_media(actor, "featured_image", &BTreeSet::from([id]))
                .await?;
        }
        Ok(())
    }

    async fn list_items(
        &self,
        caller: &Caller,
        input: ListInput,
        limit: u32,
    ) -> Result<(ListOut, String), Problem> {
        let api = &self.api;
        let actor = caller.actor();
        known_type(api, actor, &input.kind).await?;
        let params = ContentListParams {
            status: input.status.filter(|s| !s.is_empty()).map(|s| s.join(",")),
            author: input.author,
            term: input.term,
            parent: input.parent,
            q: input.q,
            order: input.order.map(Into::into),
            limit: Some(limit),
            cursor: input.cursor,
            locale: None,
        };
        let mut query = params.into_query(&input.kind)?;
        query.locale = crate::api::content::checked_locale(api, input.locale).await?;
        let page = api.content.list(actor, &query).await?;
        let map = authors(api, actor, page.items.iter().map(|i| i.author_id)).await?;
        let ids: Vec<ContentId> = page.items.iter().map(|i| i.id).collect();
        let public = self.public_urls(actor, &ids).await?;
        let states = api.content.workflow_states(actor, &page.items).await?;
        let mut items = Vec::with_capacity(page.items.len());
        for item in page.items {
            let id = item.id;
            let author = pick(&map, item.author_id)?;
            let wf = states.get(&id).cloned().unwrap_or_default();
            let allowed = ferrite_core::content::allowed_actions(actor, &item, &wf);
            let d = ContentSummaryDto::new(item, author, allowed);
            let (url, is_public) = self.url_of(d.id, d.status == "trash", public.get(&id));
            items.push(SummaryOut {
                admin_url: self.admin_url(&d.kind, d.id),
                id: d.id,
                kind: d.kind,
                title: d.title,
                slug: d.slug,
                status: d.status,
                version: d.version,
                excerpt: d.excerpt,
                parent: d.parent_id,
                author: d.author,
                created_at: d.created_at,
                updated_at: d.updated_at,
                published_at: d.published_at,
                scheduled_at: d.scheduled_at,
                url,
                public: is_public,
                locale: d.locale,
            });
        }
        let out = ListOut {
            items,
            next_cursor: page.next_cursor,
        };
        Ok((out, input.kind))
    }

    async fn create_item(&self, caller: &Caller, input: CreateInput) -> Result<Written, Problem> {
        let api = &self.api;
        let actor = caller.actor();
        known_type(api, actor, &input.kind).await?;
        if let Some(fields) = &input.fields {
            results::within_json_cap("fields", fields)?;
        }
        let converted = resolve_body(
            input.body_markdown,
            input.body_blocks,
            input.title.as_deref(),
        )?;
        let title = converted.title.or(input.title).ok_or_else(|| {
            bad_input(
                Some("title"),
                "a title is required: send `title`, or start body_markdown with a `# Heading`",
            )
        })?;
        let mut fields = input.fields.unwrap_or_default();
        if let Some(id) = input.featured_image {
            fields.insert("featured_image".into(), json!(id));
        }
        self.check_body_media(
            actor,
            converted.body.as_ref().map(|b| (b, converted.field)),
            Some(&fields),
            None,
        )
        .await?;
        let draft = ContentDraft {
            kind: input.kind,
            title,
            slug: input.slug,
            parent_id: input.parent.map(ContentId::from_uuid),
            excerpt: input.excerpt,
            body: converted
                .body
                .unwrap_or_else(|| json!({"type": "doc", "content": []})),
            fields: Value::Object(fields),
            menu_order: 0,
            terms: input
                .terms
                .unwrap_or_default()
                .into_iter()
                .map(TermId::from_uuid)
                .collect(),
            seo: input.seo.map(Into::into),
        };
        let item = if input.locale.is_some() || input.translation_of.is_some() {
            api.content
                .create_localized(
                    actor,
                    draft,
                    input.locale,
                    input.translation_of.map(ContentId::from_uuid),
                )
                .await?
        } else {
            api.content.create(actor, draft).await?
        };
        self.audit(
            caller,
            "content_create",
            "content",
            Some(item.id.to_string()),
            json!({"type": item.kind, "version": item.version}),
        )
        .await;
        Ok(self.written(actor, item, converted.notes).await)
    }

    async fn update_item(&self, caller: &Caller, input: UpdateInput) -> Result<Written, Problem> {
        let api = &self.api;
        let actor = caller.actor();
        let current = load(api, actor, &input.kind, input.id).await?;
        let id = current.id;
        if input.title.is_none()
            && input.slug.is_none()
            && input.excerpt.is_none()
            && input.body_markdown.is_none()
            && input.body_blocks.is_none()
            && input.fields.is_none()
            && input.terms.is_none()
            && input.parent.is_none()
            && input.featured_image.is_none()
            && input.seo.is_none()
        {
            return Err(bad_input(
                None,
                "nothing to change: send at least one of title, slug, excerpt, body_markdown, \
                 body_blocks, fields, terms, parent, featured_image or seo",
            ));
        }
        let converted = resolve_body(
            input.body_markdown,
            input.body_blocks,
            input.title.as_deref(),
        )?;
        if let Some(fields) = &input.fields {
            results::within_json_cap("fields", fields)?;
        }
        // The members sent replace the same members of the stored fields; `null` clears
        // a member.
        let fields = if input.fields.is_some() || input.featured_image.is_some() {
            let mut merged = current.fields.as_object().cloned().unwrap_or_default();
            merged.extend(input.fields.unwrap_or_default());
            if let Some(image) = input.featured_image {
                merged.insert("featured_image".into(), json!(image));
            }
            merged.retain(|_, v| !v.is_null());
            Some(merged)
        } else {
            None
        };
        self.check_body_media(
            actor,
            converted.body.as_ref().map(|b| (b, converted.field)),
            fields.as_ref(),
            current.fields.get("featured_image"),
        )
        .await?;
        let fields = fields.map(Value::Object);
        let patch = crate::api::dto::content::ContentPatch {
            title: converted.title.or(input.title),
            slug: input.slug,
            parent_id: input.parent,
            excerpt: input.excerpt,
            body: converted.body,
            fields,
            menu_order: None,
            terms: input.terms,
            seo: None,
            locale: None,
            translation_of: None,
        };
        let terms = api.content.terms_of(actor, id).await?;
        let mut draft = patch.apply(&current, terms);
        draft.seo = input.seo.map(Into::into);
        let relocation = ferrite_core::Relocation {
            locale: input.locale,
            translation_of: input.translation_of.map(|t| t.map(ContentId::from_uuid)),
        };
        // One transaction: the fields and the move are saved together or not at all.
        let result = api
            .content
            .update_and_relocate(actor, id, input.version, draft, relocation)
            .await;
        match result {
            Ok(item) => {
                self.audit(
                    caller,
                    "content_update",
                    "content",
                    Some(item.id.to_string()),
                    json!({"type": item.kind, "version": item.version}),
                )
                .await;
                Ok(self.written(actor, item, converted.notes).await)
            }
            Err(e) => Err(self.write_problem(actor, id, e).await),
        }
    }

    /// Runs a status transition and answers with the item.
    async fn run_transition(
        &self,
        caller: &Caller,
        input: VersionedInput,
        t: Transition,
        tool: &'static str,
        verb: &str,
    ) -> CallToolResult {
        let run = async {
            let actor = caller.actor();
            let id = load(&self.api, actor, &input.kind, input.id).await?.id;
            match apply_transition(&self.api, actor, id, input.version, t).await {
                Ok(item) => {
                    self.audit(
                        caller,
                        tool,
                        "content",
                        Some(item.id.to_string()),
                        json!({"type": item.kind, "status": item.status.as_str(),
                            "version": item.version}),
                    )
                    .await;
                    Ok(self.written(actor, item, Vec::new()).await)
                }
                Err(e) => Err(self.write_problem(actor, id, e).await),
            }
        };
        match run.await {
            Ok(out) => {
                let summary = format!(
                    "{verb} {:?} (now {}, version {}).",
                    out.title, out.status, out.version
                );
                results::ok(out.value, summary)
            }
            Err(p) => results::from_problem(p),
        }
    }

    async fn trash_item(&self, caller: &Caller, input: VersionedInput) -> CallToolResult {
        let run = async {
            let actor = caller.actor();
            let id = load(&self.api, actor, &input.kind, input.id).await?.id;
            match self.api.content.trash(actor, id, input.version).await {
                Ok(item) => {
                    self.audit(
                        caller,
                        "content_trash",
                        "content",
                        Some(item.id.to_string()),
                        json!({"type": item.kind, "status": item.status.as_str(),
                            "version": item.version}),
                    )
                    .await;
                    Ok(self.written(actor, item, Vec::new()).await)
                }
                Err(e) => Err(self.write_problem(actor, id, e).await),
            }
        };
        match run.await {
            Ok(out) => {
                let summary = format!(
                    "Moved {:?} to the trash (version {}); content_restore brings it back.",
                    out.title, out.version
                );
                results::ok(out.value, summary)
            }
            Err(p) => results::from_problem(p),
        }
    }

    async fn schedule_item(
        &self,
        caller: &Caller,
        input: ScheduleInput,
    ) -> Result<Written, Problem> {
        let actor = caller.actor();
        let at = DateTime::parse_from_rfc3339(input.at.trim())
            .map(|t| t.with_timezone(&Utc))
            .map_err(|_| {
                bad_input(
                    Some("at"),
                    "at must be an RFC 3339 time with its zone, for example 2026-12-01T09:00:00Z",
                )
            })?;
        let id = load(&self.api, actor, &input.kind, input.id).await?.id;
        match self
            .api
            .content
            .schedule(actor, id, input.version, Timestamp::from(at))
            .await
        {
            Ok(item) => {
                self.audit(
                    caller,
                    "content_schedule",
                    "content",
                    Some(item.id.to_string()),
                    json!({"type": item.kind, "at": at.to_rfc3339(), "version": item.version}),
                )
                .await;
                Ok(self.written(actor, item, Vec::new()).await)
            }
            Err(e) => Err(self.write_problem(actor, id, e).await),
        }
    }

    async fn revisions(
        &self,
        caller: &Caller,
        input: RevisionsInput,
        limit: u32,
    ) -> Result<Either, Problem> {
        let api = &self.api;
        let actor = caller.actor();
        let item = load(api, actor, &input.kind, input.id).await?;
        if let Some(rid) = input.revision {
            let rev = api
                .content
                .revision(actor, item.id, RevisionId::from_uuid(rid))
                .await?;
            let author = author_of(api, actor, rev.author_id).await?;
            let author: AuthorRefDto = author.into();
            return Ok(Either::One(RevisionOut {
                id: rev.id.as_uuid(),
                title: rev.title,
                excerpt: rev.excerpt,
                body_markdown: to_markdown(&rev.body),
                fields: rev.fields,
                note: rev.note,
                author,
                created_at: rev.created_at.as_datetime(),
            }));
        }
        let revisions = api.content.revisions(actor, item.id, limit).await?;
        let map = authors(api, actor, revisions.iter().map(|r| r.author_id)).await?;
        let items = revisions
            .into_iter()
            .map(|r| {
                let author = pick(&map, r.author_id)?;
                Ok(RevisionSummaryDto::new(r, author))
            })
            .collect::<Result<_, Problem>>()?;
        Ok(Either::List(RevisionsOut {
            item_version: item.version,
            items,
        }))
    }

    async fn restore_revision(
        &self,
        caller: &Caller,
        input: RestoreRevisionInput,
    ) -> Result<Written, Problem> {
        let actor = caller.actor();
        let id = load(&self.api, actor, &input.kind, input.id).await?.id;
        let rid = RevisionId::from_uuid(input.revision);
        match self
            .api
            .content
            .restore_revision(actor, id, rid, input.version)
            .await
        {
            Ok(item) => {
                self.audit(
                    caller,
                    "content_restore_revision",
                    "content",
                    Some(item.id.to_string()),
                    json!({"type": item.kind, "revision": input.revision,
                        "version": item.version}),
                )
                .await;
                Ok(self.written(actor, item, Vec::new()).await)
            }
            Err(e) => Err(self.write_problem(actor, id, e).await),
        }
    }

    async fn preview_link(&self, caller: &Caller, input: ItemInput) -> Result<PreviewOut, Problem> {
        let item = load(&self.api, caller.actor(), &input.kind, input.id).await?;
        if item.status == ferrite_domain::ContentStatus::Trash {
            return Err(bad_input(
                None,
                "a trashed item has no preview; restore it with content_restore first",
            ));
        }
        Ok(PreviewOut {
            url: format!("{}/_preview/{}", self.api.site_url, item.id),
            expires_at: None,
            requires_sign_in: true,
            note: format!(
                "Open it in a browser where the user is signed in to {}; the preview is not \
                 available to API tokens, and the link does not expire.",
                self.api.site_url
            ),
        })
    }

    /// The item as the `ferrite://content/{type}/{id}` resource: front matter, then the
    /// Markdown body.
    pub async fn item_markdown(
        &self,
        caller: &Caller,
        kind: &str,
        id: Uuid,
    ) -> Result<String, Problem> {
        let actor = caller.actor();
        let item = load(&self.api, actor, kind, id).await?;
        let out = self.item_out(actor, item, Vec::new()).await?;
        Ok(front_matter(&out))
    }
}

/// `out` as Markdown with a TOML front matter block between `+++` lines (the Markdown
/// importer's format). Strings are written as JSON strings, which are valid TOML basic
/// strings.
fn front_matter(out: &ItemOut) -> String {
    let q = |s: &str| serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into());
    let mut head = vec![
        format!("id = {}", q(&out.id.to_string())),
        format!("type = {}", q(&out.kind)),
        format!("title = {}", q(&out.title)),
        format!("slug = {}", q(&out.slug)),
        format!("status = {}", q(&out.status)),
        format!("version = {}", out.version),
        format!("excerpt = {}", q(&out.excerpt)),
        format!("author = {}", q(&out.author.login)),
        format!("created_at = {}", q(&out.created_at.to_rfc3339())),
        format!("updated_at = {}", q(&out.updated_at.to_rfc3339())),
    ];
    if let Some(t) = out.published_at {
        head.push(format!("published_at = {}", q(&t.to_rfc3339())));
    }
    if let Some(t) = out.scheduled_at {
        head.push(format!("scheduled_at = {}", q(&t.to_rfc3339())));
    }
    if let Some(url) = &out.url {
        head.push(format!("url = {}", q(url)));
        head.push(format!("public = {}", out.public));
    }
    head.push(format!("admin_url = {}", q(&out.admin_url)));
    if let Some(image) = &out.featured_image {
        head.push(format!("featured_image = {}", q(image)));
    }
    format!("+++\n{}\n+++\n\n{}", head.join("\n"), out.body_markdown)
}
