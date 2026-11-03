//! Release and review tools (M6-3, design 15 §12): `releases_list`, `release_get`,
//! `release_create`, `release_stage`, `release_unstage`, `release_check`,
//! `release_schedule`, `release_apply`, `release_preview_link` and `reviews_list`.
//!
//! Adapters over `ReleaseService`, `PreviewLinkService` and `ReviewService`; core checks
//! every permission (an entry's own edit and publish rights). Review decisions,
//! overrides and workflow configuration are not tools (D31): a person approves in the
//! admin.

use ferrite_core::blocks::to_markdown;
use ferrite_domain::{
    ContentId, EntryTransition, PreviewTarget, ReleaseId, ReleaseQuery, ReleaseStatus, ReviewQuery,
    ReviewStatus, StagedChange, Timestamp,
};
use rmcp::{handler::server::wrapper::Parameters, model::CallToolResult, tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{content::resolve_body, per_page};
use crate::{
    api::{
        dto::editorial::{ReleaseCheckDto, ReleaseDetailDto, ReleaseDto, ReviewDto, TransitionDto},
        problem::Problem,
    },
    mcp::{Caller, FerriteMcp, results, results::bad_input},
};

// ---- inputs ------------------------------------------------------------------------------

/// The input of `releases_list`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListInput {
    /// `open`, `scheduled`, `applied` or `failed`; leave out for all.
    #[serde(default)]
    pub status: Option<String>,
    /// Releases per page, 1 to 100 (default 20).
    #[serde(default)]
    #[schemars(range(min = 1, max = 100))]
    pub per_page: Option<u32>,
    /// `next_cursor` of the previous page.
    #[serde(default)]
    pub cursor: Option<String>,
}

/// A release named by id.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IdInput {
    /// The release's id.
    pub id: Uuid,
}

/// The input of `release_create`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateInput {
    /// 1 to 200 characters.
    pub title: String,
    /// At most 2000 characters.
    #[serde(default)]
    pub description: Option<String>,
}

/// The input of `release_stage`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StageInput {
    /// The release's id.
    pub id: Uuid,
    /// The release's `version` you last read.
    #[schemars(range(min = 1))]
    pub version: i64,
    /// The content item to stage.
    pub item: Uuid,
    /// `none` (the default: only a change), `publish` (a draft or pending item goes
    /// live with the release) or `unpublish` (a published item comes down).
    #[serde(default)]
    pub transition: Option<String>,
    /// The new title. Any of title, excerpt, slug, body_markdown, body_blocks or fields
    /// makes a staged change; the members you leave out keep the item's current values.
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub excerpt: Option<String>,
    #[serde(default)]
    pub slug: Option<String>,
    /// The new body as Markdown.
    #[serde(default)]
    pub body_markdown: Option<String>,
    /// The new body as the editor's block JSON (instead of body_markdown).
    #[serde(default)]
    pub body_blocks: Option<Value>,
    /// Custom field values replacing the item's members of the same name.
    #[serde(default)]
    pub fields: Option<serde_json::Map<String, Value>>,
}

/// The input of `release_unstage`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UnstageInput {
    pub id: Uuid,
    #[schemars(range(min = 1))]
    pub version: i64,
    pub item: Uuid,
}

/// The input of `release_schedule`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScheduleInput {
    pub id: Uuid,
    #[schemars(range(min = 1))]
    pub version: i64,
    /// When to apply, RFC 3339 with its zone (in the future, at most a year ahead);
    /// `null` takes the release off the schedule.
    pub at: Option<String>,
}

/// The input of `release_apply`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyInput {
    pub id: Uuid,
    /// The release's `version` you last read.
    #[schemars(range(min = 1))]
    pub version: i64,
}

/// The input of `release_preview_link`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LinkInput {
    pub id: Uuid,
    /// 1 to 168 hours (default 24).
    #[serde(default)]
    #[schemars(range(min = 1, max = 168))]
    pub expires_in_hours: Option<u32>,
}

/// The input of `reviews_list`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewsInput {
    /// `true`: only reviews waiting for you at their current step.
    #[serde(default)]
    pub assigned_to_me: Option<bool>,
    /// `true`: only reviews you submitted.
    #[serde(default)]
    pub submitted_by_me: Option<bool>,
    #[serde(default)]
    #[schemars(range(min = 1, max = 100))]
    pub per_page: Option<u32>,
    #[serde(default)]
    pub cursor: Option<String>,
}

fn transition(t: Option<&str>) -> Result<EntryTransition, Problem> {
    match t.unwrap_or("none") {
        "none" => Ok(EntryTransition::None),
        "publish" => Ok(EntryTransition::Publish),
        "unpublish" => Ok(EntryTransition::Unpublish),
        _ => Err(bad_input(
            Some("transition"),
            "transition is none, publish or unpublish",
        )),
    }
}

/// The release detail with staged bodies as Markdown.
fn detail_value(d: ReleaseDetailDto) -> Value {
    let mut v = serde_json::to_value(d).unwrap_or(Value::Null);
    if let Some(entries) = v.get_mut("entries").and_then(Value::as_array_mut) {
        for e in entries {
            if let Some(changes) = e.get_mut("changes").and_then(Value::as_object_mut)
                && let Some(body) = changes.remove("body")
            {
                changes.insert("body_markdown".into(), json!(to_markdown(&body)));
            }
        }
    }
    v
}

// ---- tools -------------------------------------------------------------------------------

#[tool_router(router = release_tools, vis = "pub(crate)")]
impl FerriteMcp {
    /// Lists the releases you may see (your own; editors and admins see all), newest
    /// first: title, status, schedule and entry count. A release is a set of content
    /// changes applied together, now or at a scheduled time.
    #[tool(annotations(
        title = "List releases",
        read_only_hint = true,
        open_world_hint = false
    ))]
    pub async fn releases_list(
        &self,
        caller: Caller,
        Parameters(input): Parameters<ListInput>,
    ) -> CallToolResult {
        let limit = match per_page(input.per_page) {
            Ok(n) => n,
            Err(e) => return e,
        };
        let status = match input.status.as_deref().map(str::parse::<ReleaseStatus>) {
            None => None,
            Some(Ok(s)) => Some(s),
            Some(Err(_)) => {
                return results::invalid(
                    Some("status"),
                    "status is open, scheduled, applied or failed",
                );
            }
        };
        let q = ReleaseQuery {
            status,
            created_by: None,
            cursor: input.cursor,
            limit,
        };
        match self
            .api
            .editorial
            .releases
            .list(caller.actor(), q, false)
            .await
        {
            Ok((page, counts)) => {
                let items: Vec<ReleaseDto> = page
                    .items
                    .into_iter()
                    .map(|r| {
                        let n = counts.iter().find(|(i, _)| *i == r.id).map_or(0, |c| c.1);
                        ReleaseDto::new(r, n)
                    })
                    .collect();
                let summary = format!("{} releases.", items.len());
                results::ok(
                    json!({"items": items, "next_cursor": page.next_cursor}),
                    summary,
                )
            }
            Err(e) => results::from_core(e),
        }
    }

    /// Returns a release with its entries: each item, its action (`publish`,
    /// `unpublish` or none), the staged change (body as Markdown), whether the item moved
    /// since it was staged (`conflict`: rebase in the admin) and its review.
    #[tool(annotations(title = "Get release", read_only_hint = true, open_world_hint = false))]
    pub async fn release_get(
        &self,
        caller: Caller,
        Parameters(input): Parameters<IdInput>,
    ) -> CallToolResult {
        match self
            .api
            .editorial
            .releases
            .get_for(caller.actor(), ReleaseId::from_uuid(input.id))
            .await
        {
            Ok(v) => {
                let d = ReleaseDetailDto::from(v);
                let summary = format!("Release {:?} ({}).", d.release.title, d.release.status);
                results::ok(detail_value(d), summary)
            }
            Err(e) => results::from_core(e),
        }
    }

    /// Creates an empty, open release. Stage changes into it with release_stage.
    #[tool(annotations(
        title = "Create release",
        read_only_hint = false,
        destructive_hint = false,
        idempotent_hint = false,
        open_world_hint = false
    ))]
    pub async fn release_create(
        &self,
        caller: Caller,
        Parameters(input): Parameters<CreateInput>,
    ) -> CallToolResult {
        match self
            .api
            .editorial
            .releases
            .create(
                caller.actor(),
                ferrite_core::release::NewRelease {
                    title: input.title,
                    description: input.description.unwrap_or_default(),
                },
            )
            .await
        {
            Ok(r) => {
                self.audit(
                    &caller,
                    "release_create",
                    "release",
                    Some(r.id.to_string()),
                    json!({}),
                )
                .await;
                let summary = format!("Created release {:?} (version {}).", r.title, r.version);
                results::ok(ReleaseDto::new(r, 0), summary)
            }
            Err(e) => results::from_core(e),
        }
    }

    /// Adds or replaces the entry of one item in an open release: a staged change
    /// (title, excerpt, slug, body, fields; nothing is live until the release is applied)
    /// and/or a `publish`/`unpublish` transition. Use this to change a published page of
    /// a type with an approval workflow: such pages cannot be edited in place. Send the
    /// release's `version`.
    #[tool(annotations(
        title = "Stage in release",
        read_only_hint = false,
        destructive_hint = false,
        idempotent_hint = true,
        open_world_hint = false
    ))]
    pub async fn release_stage(
        &self,
        caller: Caller,
        Parameters(input): Parameters<StageInput>,
    ) -> CallToolResult {
        match self.stage(&caller, input).await {
            Ok((d, summary)) => results::ok(detail_value(d), summary),
            Err(p) => results::from_problem(p),
        }
    }

    /// Removes one item's entry from an open release. Send the release's `version`.
    #[tool(annotations(
        title = "Unstage from release",
        read_only_hint = false,
        destructive_hint = false,
        idempotent_hint = false,
        open_world_hint = false
    ))]
    pub async fn release_unstage(
        &self,
        caller: Caller,
        Parameters(input): Parameters<UnstageInput>,
    ) -> CallToolResult {
        let id = ReleaseId::from_uuid(input.id);
        let svc = &self.api.editorial.releases;
        match svc
            .unstage(
                caller.actor(),
                id,
                input.version,
                ContentId::from_uuid(input.item),
            )
            .await
        {
            Ok(r) => {
                self.audit(
                    &caller,
                    "release_unstage",
                    "release",
                    Some(id.to_string()),
                    json!({"item": input.item.to_string()}),
                )
                .await;
                self.release_value(&caller, r.id, "Unstaged").await
            }
            Err(e) => results::from_core(e),
        }
    }

    /// Checks what applying a release now would do, entry by entry (`ok`, `conflict`,
    /// `needs_approval`, `forbidden`, `invalid`), without writing anything.
    #[tool(annotations(
        title = "Check release",
        read_only_hint = true,
        open_world_hint = false
    ))]
    pub async fn release_check(
        &self,
        caller: Caller,
        Parameters(input): Parameters<IdInput>,
    ) -> CallToolResult {
        match self
            .api
            .editorial
            .releases
            .check(caller.actor(), ReleaseId::from_uuid(input.id))
            .await
        {
            Ok(c) => {
                let d = ReleaseCheckDto::from(c);
                let summary = if d.clean {
                    "Every entry can be applied.".to_owned()
                } else {
                    "Some entries cannot be applied; see `entries`.".to_owned()
                };
                results::ok(d, summary)
            }
            Err(e) => results::from_core(e),
        }
    }

    /// Schedules a release to be applied at `at` (its items are then locked against
    /// other changes), or takes it off the schedule with `at: null`. Scheduling needs a
    /// clean release_check. Send the release's `version`.
    #[tool(annotations(
        title = "Schedule release",
        read_only_hint = false,
        destructive_hint = false,
        idempotent_hint = true,
        open_world_hint = false
    ))]
    pub async fn release_schedule(
        &self,
        caller: Caller,
        Parameters(input): Parameters<ScheduleInput>,
    ) -> CallToolResult {
        let id = ReleaseId::from_uuid(input.id);
        let svc = &self.api.editorial.releases;
        let result = match input.at {
            None => svc.unschedule(caller.actor(), id, input.version).await,
            Some(at) => {
                let Ok(at) = chrono::DateTime::parse_from_rfc3339(&at) else {
                    return results::invalid(Some("at"), "at is an RFC 3339 time with its zone");
                };
                let at = Timestamp::from(at.with_timezone(&chrono::Utc));
                svc.schedule(caller.actor(), id, input.version, at, None)
                    .await
            }
        };
        match result {
            Ok(r) => {
                self.audit(
                    &caller,
                    "release_schedule",
                    "release",
                    Some(id.to_string()),
                    json!({"status": r.status.as_str()}),
                )
                .await;
                self.release_value(&caller, r.id, "Saved").await
            }
            Err(e) => results::from_core(e),
        }
    }

    /// Applies a release now: every entry is written in one transaction (all or
    /// nothing), and what it publishes goes live at once. Ask the user first. Approvals
    /// a workflow needs are given by people in the admin; this tool cannot override
    /// them. Send the release's `version`.
    #[tool(annotations(
        title = "Apply release",
        read_only_hint = false,
        destructive_hint = true,
        idempotent_hint = false,
        open_world_hint = false
    ))]
    pub async fn release_apply(
        &self,
        caller: Caller,
        Parameters(input): Parameters<ApplyInput>,
    ) -> CallToolResult {
        let id = ReleaseId::from_uuid(input.id);
        match self
            .api
            .editorial
            .releases
            .apply(caller.actor(), id, input.version, None)
            .await
        {
            Ok(r) => {
                self.audit(
                    &caller,
                    "release_apply",
                    "release",
                    Some(id.to_string()),
                    json!({}),
                )
                .await;
                self.release_value(&caller, r.id, "Applied").await
            }
            Err(e) => results::from_core(e),
        }
    }

    /// Returns a signed link that shows the whole site as the release will make it, in
    /// any browser and without signing in (until it expires: 1 to 168 hours, default 24).
    /// Give it to the user.
    #[tool(annotations(
        title = "Release preview link",
        read_only_hint = false,
        destructive_hint = false,
        idempotent_hint = false,
        open_world_hint = false
    ))]
    pub async fn release_preview_link(
        &self,
        caller: Caller,
        Parameters(input): Parameters<LinkInput>,
    ) -> CallToolResult {
        let target = PreviewTarget::Release(ReleaseId::from_uuid(input.id));
        match self
            .signed_link(&caller, target, input.expires_in_hours)
            .await
        {
            Ok(v) => {
                let summary = format!("Preview: {}", v["url"].as_str().unwrap_or_default());
                results::ok(v, summary)
            }
            Err(e) => results::from_core(e),
        }
    }

    /// Lists open reviews (content waiting for approval in a workflow), oldest first:
    /// the item, the step it waits for and who submitted it. Read only: people approve
    /// or request changes in the admin.
    #[tool(annotations(title = "List reviews", read_only_hint = true, open_world_hint = false))]
    pub async fn reviews_list(
        &self,
        caller: Caller,
        Parameters(input): Parameters<ReviewsInput>,
    ) -> CallToolResult {
        let limit = match per_page(input.per_page) {
            Ok(n) => n,
            Err(e) => return e,
        };
        let me = caller.actor().user;
        let q = ReviewQuery {
            status: Some(ReviewStatus::InReview),
            assignee: input.assigned_to_me.unwrap_or(false).then_some(me),
            submitted_by: input.submitted_by_me.unwrap_or(false).then_some(me),
            kind: None,
            cursor: input.cursor,
            limit,
        };
        match self.api.editorial.reviews.list(caller.actor(), q).await {
            Ok(page) => {
                let mut items = Vec::with_capacity(page.items.len());
                for r in page.items {
                    let mut d = ReviewDto::from(r);
                    if let Ok(Some(item)) = self
                        .api
                        .store
                        .read()
                        .content()
                        .get(ContentId::from_uuid(d.item_id))
                        .await
                    {
                        d.item_title = Some(item.title);
                        d.item_type = Some(item.kind);
                    }
                    items.push(d);
                }
                let summary = format!("{} open reviews.", items.len());
                results::ok(
                    json!({"items": items, "next_cursor": page.next_cursor}),
                    summary,
                )
            }
            Err(e) => results::from_core(e),
        }
    }
}

impl FerriteMcp {
    async fn release_value(&self, caller: &Caller, id: ReleaseId, verb: &str) -> CallToolResult {
        match self
            .api
            .editorial
            .releases
            .get_for(caller.actor(), id)
            .await
        {
            Ok(v) => {
                let d = ReleaseDetailDto::from(v);
                let summary = format!(
                    "{verb} release {:?} (now {}, version {}).",
                    d.release.title, d.release.status, d.release.version
                );
                results::ok(detail_value(d), summary)
            }
            Err(e) => results::from_core(e),
        }
    }

    /// A new signed link to `target` as JSON (`url`, `expires_at`, `note`).
    pub(crate) async fn signed_link(
        &self,
        caller: &Caller,
        target: PreviewTarget,
        hours: Option<u32>,
    ) -> Result<Value, ferrite_core::CoreError> {
        let (link, token) = self
            .api
            .editorial
            .preview_links
            .create(caller.actor(), target, hours)
            .await?;
        self.audit(
            caller,
            "preview_link",
            target.kind().as_str(),
            Some(target.id().to_string()),
            json!({"link": link.id.to_string()}),
        )
        .await;
        let expires: chrono::DateTime<chrono::Utc> = link.expires_at.into();
        Ok(json!({
            "url": format!("{}/_preview/link/{token}", self.api.site_url),
            "expires_at": expires.to_rfc3339(),
            "requires_sign_in": false,
            "note": "Anyone with the link can open the preview until it expires; revoke it in the admin.",
        }))
    }

    async fn stage(
        &self,
        caller: &Caller,
        input: StageInput,
    ) -> Result<(ReleaseDetailDto, String), Problem> {
        let actor = caller.actor();
        let transition = transition(input.transition.as_deref())?;
        let item_id = ContentId::from_uuid(input.item);
        let has_change = input.title.is_some()
            || input.excerpt.is_some()
            || input.slug.is_some()
            || input.body_markdown.is_some()
            || input.body_blocks.is_some()
            || input.fields.is_some();
        let changes = if has_change {
            let current = self.api.content.get_for(actor, item_id).await?;
            let body = resolve_body(
                input.body_markdown,
                input.body_blocks,
                input.title.as_deref(),
            )?;
            let mut fields = current.fields.as_object().cloned().unwrap_or_default();
            fields.extend(input.fields.unwrap_or_default());
            fields.retain(|_, v| !v.is_null());
            let terms = self.api.content.terms_of(actor, item_id).await?;
            Some(StagedChange {
                title: input.title.or(body.title).unwrap_or(current.title),
                slug: input.slug,
                parent_id: current.parent_id,
                excerpt: input.excerpt.or(Some(current.excerpt)),
                body: body.body.unwrap_or(current.body),
                fields: Value::Object(fields),
                menu_order: current.menu_order,
                terms,
                seo: None,
            })
        } else {
            None
        };
        let id = ReleaseId::from_uuid(input.id);
        let r = self
            .api
            .editorial
            .releases
            .stage(
                actor,
                id,
                input.version,
                ferrite_core::release::EntryInput {
                    item: item_id,
                    transition,
                    changes,
                },
            )
            .await?;
        self.audit(
            caller,
            "release_stage",
            "release",
            Some(id.to_string()),
            json!({"item": input.item.to_string(), "transition": TransitionDto::from(transition)}),
        )
        .await;
        let view = self.api.editorial.releases.get_for(actor, r.id).await?;
        let d = ReleaseDetailDto::from(view);
        let summary = format!(
            "Staged in release {:?} (version {}, {} entries).",
            d.release.title,
            d.release.version,
            d.entries.len()
        );
        Ok((d, summary))
    }
}
