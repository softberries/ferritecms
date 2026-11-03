//! Content quality tools (M6-4, design 16 §10): `content_check` and `health_report`.
//! Read-only: no tool suppresses a finding or starts a health run (D16).
//!
//! Adapters over [`QualityService`](ferrite_core::quality::QualityService): core checks
//! `content:read` and read access to the item (`content_check`), or `edit_others`
//! (`health_report`). The words of each finding (a sentence and a fix) are written here,
//! in English; core stores rule ids only.

use ferrite_core::quality::{CheckDraft, Finding};
use ferrite_domain::{ContentId, FindingQuery};
use rmcp::{handler::server::wrapper::Parameters, model::CallToolResult, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use super::{content::resolve_body, per_page};
use crate::{
    api::dto::quality::{
        CountsDto, GateDto, HealthFindingDto, HealthRunDto, LocationDto, SuppressionDto,
    },
    mcp::{Caller, FerriteMcp, results},
};

/// A sentence about a finding of `rule`, with what was found.
pub fn message(rule: &str, context: Option<&str>) -> String {
    let c = context.unwrap_or("");
    match rule {
        "image-alt-missing" => "An image has no alt text.".into(),
        "image-alt-empty" => "An image has an empty alt text but is not marked decorative.".into(),
        "image-alt-filename" => format!("An image's alt text looks like a file name: \"{c}\"."),
        "heading-empty" => "A heading has no text.".into(),
        "heading-level-skipped" => format!("The heading \"{c}\" skips a level."),
        "link-text-empty" => "A link has no text.".into(),
        "link-text-vague" => format!("The link text \"{c}\" does not say where it leads."),
        "link-text-url" => format!("A link's text is a bare URL: {c}."),
        "table-header-missing" => "A table has no header row or column.".into(),
        "table-header-empty" => "A table header cell is empty.".into(),
        "html-unchecked" => "A raw HTML block is not checked.".into(),
        "media-missing" => "An image points at media that is missing or trashed.".into(),
        "featured-alt-missing" => "The featured image's media item has no alt text.".into(),
        "link-internal-broken" => format!("The link \"{c}\" leads to a page that does not exist."),
        "link-internal-redirected" => format!("The link \"{c}\" leads to a page that redirects."),
        "link-anchor-missing" => format!("The link {c} names a heading that is not on the page."),
        "link-external-broken" => format!("The external link \"{c}\" is broken."),
        "orphan" => "Nothing on the site links to this item.".into(),
        "redirect-chain" => format!("A redirect leads to another redirect: {c}."),
        "redirect-loop" => format!("Redirects lead back to where they started: {c}."),
        "redirect-target-broken" => format!("A redirect leads to a page that does not exist: {c}."),
        "stale" => "This item has not been edited for a long time.".into(),
        "check-truncated" => "The item is too large or too deep to check completely.".into(),
        "findings-truncated" => "The run found too many problems to list them all.".into(),
        other => format!("Finding {other}."),
    }
}

/// How to fix a finding of `rule`.
pub fn fix(rule: &str) -> &'static str {
    match rule {
        "image-alt-missing" | "image-alt-empty" => {
            "Describe what the image shows in its alt text (![description](...)), or mark it \
             decorative in the admin editor if it adds nothing."
        }
        "image-alt-filename" => "Replace the alt text with a short description of the image.",
        "heading-empty" => "Give the heading text, or remove it.",
        "heading-level-skipped" => {
            "Use the next heading level (## after the title, then ###), without skipping one."
        }
        "link-text-empty" | "link-text-vague" | "link-text-url" => {
            "Make the link text say where it leads, e.g. [the pricing page](/pricing/)."
        }
        "table-header-missing" | "table-header-empty" => {
            "Give the table a header row (| Name | Price | then | --- | --- |) with text in every header cell."
        }
        "html-unchecked" => "Check the HTML block by hand, or use Markdown instead.",
        "media-missing" => "Use a media id from media_list, or remove the image.",
        "featured-alt-missing" => "Set the media item's alt text with media_update.",
        "link-internal-broken" | "link-anchor-missing" | "link-external-broken" => {
            "Fix the link's address or remove the link."
        }
        "link-internal-redirected" => "Link straight to the final address.",
        "orphan" => "Link to it from another page or a menu.",
        "redirect-chain" | "redirect-loop" | "redirect-target-broken" => {
            "Fix the redirects in the admin (Structure, Redirects)."
        }
        "stale" => "Review the item and update it, or leave it if it is still right.",
        _ => "",
    }
}

/// One finding as the tools return it.
#[derive(Debug, Serialize)]
pub struct ToolFinding {
    pub rule: String,
    pub severity: String,
    pub category: String,
    pub wcag: Vec<&'static str>,
    pub message: String,
    pub context: Option<String>,
    pub fix: &'static str,
    pub location: Option<Value>,
    pub key: String,
    pub suppressed: Option<SuppressionDto>,
}

fn tool_finding(f: &Finding, suppressed: Option<SuppressionDto>) -> ToolFinding {
    ToolFinding {
        rule: f.rule.id.to_owned(),
        severity: f.rule.severity.as_str().to_owned(),
        category: f.rule.category.as_str().to_owned(),
        wcag: f.rule.wcag.to_vec(),
        message: message(f.rule.id, f.context.as_deref()),
        context: f.context.clone(),
        fix: fix(f.rule.id),
        location: serde_json::to_value(LocationDto::from(&f.location)).ok(),
        key: f.key.clone(),
        suppressed,
    }
}

/// The input of `content_check`: an item by `id`, or a draft by `type`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckInput {
    /// A stored item to check (its stored title, body and fields). Send either `id` or
    /// `type`.
    #[serde(default)]
    pub id: Option<Uuid>,
    /// The content type of a draft to check: `post`, `page` or a custom type.
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    /// The draft's body as Markdown (as for content_create).
    #[serde(default)]
    pub body_markdown: Option<String>,
    /// The draft's body as block JSON.
    #[serde(default)]
    pub body_blocks: Option<Value>,
    /// The draft's custom fields (rich-text fields are checked too).
    #[serde(default)]
    pub fields: Option<Map<String, Value>>,
    /// The draft's locale (BCP 47); the site's default when absent.
    #[serde(default)]
    pub locale: Option<String>,
}

/// What `content_check` returns.
#[derive(Debug, Serialize)]
pub struct CheckOutput {
    pub findings: Vec<ToolFinding>,
    pub counts: CountsDto,
    pub gate: GateDto,
}

/// The input of `health_report`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReportInput {
    /// `accessibility`, `links`, `media`, `site` or `freshness`.
    #[serde(default)]
    pub category: Option<String>,
    /// One rule id, e.g. `image-alt-missing`.
    #[serde(default)]
    pub rule: Option<String>,
    /// `error`, `warning` or `notice`.
    #[serde(default)]
    pub severity: Option<String>,
    /// Only findings on items of this content type.
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
    /// Only findings on items in this locale.
    #[serde(default)]
    pub locale: Option<String>,
    /// Findings per page, 1 to 100 (default 20).
    #[serde(default)]
    #[schemars(range(min = 1, max = 100))]
    pub per_page: Option<u32>,
    /// `next_cursor` of the previous page of the same filters.
    #[serde(default)]
    pub cursor: Option<String>,
}

/// What `health_report` returns.
#[derive(Debug, Serialize)]
pub struct ReportOutput {
    pub summary: Value,
    pub findings: Vec<Value>,
    pub next_cursor: Option<String>,
}

#[tool_router(router = quality_tools, vis = "pub(crate)")]
impl FerriteMcp {
    /// Checks a content item, or a draft you are about to write, for accessibility and
    /// link problems: images without alt text, empty or skipped headings, vague link
    /// text ("click here"), tables without headers, missing media and broken links on
    /// this site. Each finding has its rule, severity, WCAG criteria, a sentence, what
    /// was found and how to fix it. Run it before content_publish and fix the errors:
    /// when the type's publish gate is on (`gate.mode` = `errors`), publishing is
    /// refused while accessibility errors remain. Read-only.
    #[tool(annotations(
        title = "Check content",
        read_only_hint = true,
        open_world_hint = false
    ))]
    pub async fn content_check(
        &self,
        caller: Caller,
        Parameters(input): Parameters<CheckInput>,
    ) -> CallToolResult {
        let (kind, draft) = match (input.id, input.kind) {
            (Some(_), Some(_)) | (None, None) => {
                return results::invalid(Some("id"), "send either id or type");
            }
            (Some(id), None) => {
                let id = ContentId::from_uuid(id);
                let item = match self.api.content.get_for(caller.actor(), id).await {
                    Ok(i) if i.kind != ferrite_domain::MEDIA_KIND => i,
                    Ok(_) => return results::from_core(ferrite_core::CoreError::NotFound),
                    Err(e) => return results::from_core(e),
                };
                let draft = CheckDraft {
                    id: Some(id),
                    title: item.title.clone(),
                    body: item.body.clone(),
                    fields: item.fields.clone(),
                    locale: None,
                };
                (item.kind, draft)
            }
            (None, Some(kind)) => {
                let body = match resolve_body(
                    input.body_markdown,
                    input.body_blocks,
                    input.title.as_deref(),
                ) {
                    Ok(b) => b,
                    Err(p) => return results::from_problem(p),
                };
                let title = input.title.or(body.title).unwrap_or_default();
                let draft = CheckDraft {
                    id: None,
                    title,
                    body: body
                        .body
                        .unwrap_or_else(|| json!({"type": "doc", "content": []})),
                    fields: input.fields.map(Value::Object).unwrap_or_else(|| json!({})),
                    locale: input.locale,
                };
                (kind, draft)
            }
        };
        let report = match self
            .api
            .quality
            .check(caller.actor(), &kind, draft, self.api.probe.as_deref())
            .await
        {
            Ok(r) => r,
            Err(e) => return results::from_core(e),
        };
        let out = CheckOutput {
            findings: report
                .findings
                .iter()
                .map(|c| tool_finding(&c.finding, c.suppressed.as_ref().map(Into::into)))
                .collect(),
            counts: report.counts.into(),
            gate: report.gate.into(),
        };
        let summary = format!(
            "{} errors, {} warnings, {} notices{}{}.",
            out.counts.error,
            out.counts.warning,
            out.counts.notice,
            if out.counts.suppressed > 0 {
                format!(" ({} suppressed)", out.counts.suppressed)
            } else {
                String::new()
            },
            if report.gate.blocked {
                "; publishing is blocked until the accessibility errors are fixed"
            } else {
                ""
            }
        );
        results::ok(out, summary)
    }

    /// The site's content health report: when it last ran, totals per rule and
    /// severity, and one page of findings (broken links, orphan pages, images without
    /// alt text, redirect chains, items not edited for a long time), each with the item
    /// it is on. Filter by `category`, `rule`, `severity`, `type` or `locale`. Runs are
    /// daily; a person can start one in the admin (Content health). Needs an editor or
    /// administrator. Read-only.
    #[tool(annotations(
        title = "Content health report",
        read_only_hint = true,
        open_world_hint = false
    ))]
    pub async fn health_report(
        &self,
        caller: Caller,
        Parameters(input): Parameters<ReportInput>,
    ) -> CallToolResult {
        let limit = match per_page(input.per_page) {
            Ok(n) => n,
            Err(e) => return e,
        };
        let summary = match self.api.quality.summary(caller.actor()).await {
            Ok(s) => s,
            Err(e) => return results::from_core(e),
        };
        let page = match self
            .api
            .quality
            .findings(
                caller.actor(),
                FindingQuery {
                    run: None,
                    category: input.category,
                    rule: input.rule,
                    severity: input.severity,
                    item_type: input.kind,
                    locale: input.locale,
                    limit,
                    cursor: input.cursor,
                },
            )
            .await
        {
            Ok(p) => p,
            Err(ferrite_core::CoreError::Invalid(_)) => {
                return results::invalid(
                    Some("cursor"),
                    "pass next_cursor of the same query unchanged",
                );
            }
            Err(e) => return results::from_core(e),
        };
        let findings: Vec<Value> = page
            .items
            .iter()
            .map(|h| {
                let dto = HealthFindingDto::from(h);
                let mut v = serde_json::to_value(&dto).unwrap_or(Value::Null);
                if let Value::Object(m) = &mut v {
                    m.insert(
                        "message".into(),
                        Value::String(message(&dto.rule, dto.context.as_deref())),
                    );
                    m.insert("fix".into(), Value::String(fix(&dto.rule).into()));
                }
                v
            })
            .collect();
        let text = match &summary.last {
            None => "No content health run has finished yet.".to_owned(),
            Some(r) => format!(
                "Last run {}: {}; {}.",
                r.finished_at
                    .map(|t| t.as_datetime().to_rfc3339())
                    .unwrap_or_default(),
                r.totals["severity"],
                results::page_summary(findings.len(), "finding", page.next_cursor.is_some())
            ),
        };
        let out = ReportOutput {
            summary: json!({
                "last": summary.last.as_ref().map(HealthRunDto::from),
                "running": summary.running.as_ref().map(HealthRunDto::from),
                "next_at": summary.next_at.as_datetime(),
                "external_links": summary.external_links,
            }),
            findings,
            next_cursor: page.next_cursor,
        };
        results::ok(out, text)
    }
}
