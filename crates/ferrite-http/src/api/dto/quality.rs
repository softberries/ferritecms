//! Content quality bodies (M6-4, design 16 §10): checks, findings, suppressions and the
//! health report.

use chrono::{DateTime, Utc};
use ferrite_core::quality::{CheckReport, CheckedFinding, Counts, Finding, GateState, Location};
use ferrite_domain::{QualityGate, Suppression};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;
use uuid::Uuid;

/// Every rule id (`ferrite_core::quality::RULES`, design 16 §3); a test keeps the two
/// equal.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum RuleId {
    ImageAltMissing,
    ImageAltEmpty,
    ImageAltFilename,
    HeadingEmpty,
    HeadingLevelSkipped,
    LinkTextEmpty,
    LinkTextVague,
    LinkTextUrl,
    TableHeaderMissing,
    TableHeaderEmpty,
    HtmlUnchecked,
    MediaMissing,
    FeaturedAltMissing,
    LinkInternalBroken,
    LinkInternalRedirected,
    LinkAnchorMissing,
    LinkExternalBroken,
    Orphan,
    RedirectChain,
    RedirectLoop,
    RedirectTargetBroken,
    Stale,
    CheckTruncated,
    FindingsTruncated,
}

/// The severity of a rule.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SeverityDto {
    Error,
    Warning,
    Notice,
}

/// The category of a rule.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CategoryDto {
    Accessibility,
    Links,
    Media,
    Site,
    Freshness,
}

/// A type's publish gate.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum QualityGateDto {
    /// Checks only report.
    Off,
    /// Accessibility errors block publishing.
    Errors,
}

impl From<QualityGate> for QualityGateDto {
    fn from(g: QualityGate) -> Self {
        match g {
            QualityGate::Off => Self::Off,
            QualityGate::Errors => Self::Errors,
        }
    }
}

impl From<QualityGateDto> for QualityGate {
    fn from(g: QualityGateDto) -> Self {
        match g {
            QualityGateDto::Off => Self::Off,
            QualityGateDto::Errors => Self::Errors,
        }
    }
}

/// Where a finding is.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct LocationDto {
    /// `title`, `body`, `field`, `featured_image`, `item` or `redirect`.
    pub kind: &'static str,
    /// For `body` and `field`: the child indices from the document root.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<Vec<u32>>,
    /// For `field`: the field's path, e.g. `chapters[1].summary`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    /// For `redirect`: the redirect's id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redirect_id: Option<Uuid>,
}

impl From<&Location> for LocationDto {
    fn from(l: &Location) -> Self {
        let plain = |kind| Self {
            kind,
            path: None,
            field: None,
            redirect_id: None,
        };
        match l {
            Location::Title => plain("title"),
            Location::Body { path } => Self {
                path: Some(path.clone()),
                ..plain("body")
            },
            Location::Field { name, path } => Self {
                path: Some(path.clone()),
                field: Some(name.clone()),
                ..plain("field")
            },
            Location::FeaturedImage => plain("featured_image"),
            Location::Item => plain("item"),
            Location::Redirect { id } => Self {
                redirect_id: Some(id.as_uuid()),
                ..plain("redirect")
            },
        }
    }
}

/// Who accepted a finding, and why.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SuppressionDto {
    #[schema(value_type = RuleId)]
    pub rule: String,
    /// `""`: every finding of the rule on the item.
    pub key: String,
    pub reason: String,
    pub user_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

impl From<&Suppression> for SuppressionDto {
    fn from(s: &Suppression) -> Self {
        Self {
            rule: s.rule.clone(),
            key: s.key.clone(),
            reason: s.reason.clone(),
            user_id: s.user_id.map(|u| u.as_uuid()),
            created_at: s.created_at.as_datetime(),
        }
    }
}

/// One finding.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct FindingDto {
    #[schema(value_type = RuleId)]
    pub rule: &'static str,
    #[schema(value_type = SeverityDto)]
    pub severity: &'static str,
    #[schema(value_type = CategoryDto)]
    pub category: &'static str,
    /// WCAG 2.2 success criteria, e.g. `1.1.1`.
    pub wcag: Vec<&'static str>,
    pub location: LocationDto,
    /// The suppression key of this finding.
    pub key: String,
    /// What was found: link text, alt, href or heading text (at most 120 characters).
    pub context: Option<String>,
    /// Rule-specific facts, e.g. `{"href": "/x/", "status": 404}`.
    #[schema(value_type = Object)]
    pub detail: Value,
    /// Set when a person accepted this finding: it counts nowhere.
    pub suppressed: Option<SuppressionDto>,
}

impl FindingDto {
    pub fn new(f: &Finding, suppressed: Option<&Suppression>) -> Self {
        Self {
            rule: f.rule.id,
            severity: f.rule.severity.as_str(),
            category: f.rule.category.as_str(),
            wcag: f.rule.wcag.to_vec(),
            location: (&f.location).into(),
            key: f.key.clone(),
            context: f.context.clone(),
            detail: f.detail.clone(),
            suppressed: suppressed.map(Into::into),
        }
    }
}

impl From<&CheckedFinding> for FindingDto {
    fn from(c: &CheckedFinding) -> Self {
        Self::new(&c.finding, c.suppressed.as_ref())
    }
}

/// The `findings` member of a `content-quality` problem.
pub fn findings_json(findings: &[Finding]) -> Value {
    serde_json::to_value(
        findings
            .iter()
            .map(|f| FindingDto::new(f, None))
            .collect::<Vec<_>>(),
    )
    .unwrap_or(Value::Array(Vec::new()))
}

/// Unsuppressed findings per severity, and the suppressed ones.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
pub struct CountsDto {
    pub error: u32,
    pub warning: u32,
    pub notice: u32,
    pub suppressed: u32,
}

impl From<Counts> for CountsDto {
    fn from(c: Counts) -> Self {
        Self {
            error: c.error,
            warning: c.warning,
            notice: c.notice,
            suppressed: c.suppressed,
        }
    }
}

/// The type's publish gate and whether it refuses this draft now.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
pub struct GateDto {
    pub mode: QualityGateDto,
    pub blocked: bool,
}

impl From<GateState> for GateDto {
    fn from(g: GateState) -> Self {
        Self {
            mode: g.mode.into(),
            blocked: g.blocked,
        }
    }
}

/// `POST /content/{type}/check`: the editor's working draft.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckRequestDto {
    /// The stored item this is a draft of (its suppressions and locale apply).
    pub id: Option<Uuid>,
    #[serde(default)]
    pub title: String,
    /// The block tree.
    #[schema(value_type = Object)]
    pub body: Value,
    /// Custom fields (rich-text fields are checked; `featured_image` too).
    #[serde(default)]
    #[schema(value_type = Option<Object>)]
    pub fields: Option<Value>,
    /// The locale of a new item; the site's default when absent.
    pub locale: Option<String>,
}

/// What a check found.
#[derive(Debug, Serialize, ToSchema)]
pub struct CheckResponseDto {
    pub findings: Vec<FindingDto>,
    pub counts: CountsDto,
    pub gate: GateDto,
}

impl From<CheckReport> for CheckResponseDto {
    fn from(r: CheckReport) -> Self {
        Self {
            findings: r.findings.iter().map(Into::into).collect(),
            counts: r.counts.into(),
            gate: r.gate.into(),
        }
    }
}

/// `POST /content/{type}/{id}/suppressions`.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SuppressInput {
    #[schema(value_type = RuleId)]
    pub rule: String,
    /// One finding's key, or `""` for every finding of the rule on the item.
    #[serde(default)]
    pub key: String,
    /// Why (1 to 500 characters).
    pub reason: String,
}

/// An item's suppressions.
#[derive(Debug, Serialize, ToSchema)]
pub struct SuppressionList {
    pub items: Vec<SuppressionDto>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_enum_matches_core_catalogue() {
        let names: Vec<String> = ferrite_core::quality::RULES
            .iter()
            .map(|r| r.id.to_owned())
            .collect();
        let ours: Vec<String> = [
            RuleId::ImageAltMissing,
            RuleId::ImageAltEmpty,
            RuleId::ImageAltFilename,
            RuleId::HeadingEmpty,
            RuleId::HeadingLevelSkipped,
            RuleId::LinkTextEmpty,
            RuleId::LinkTextVague,
            RuleId::LinkTextUrl,
            RuleId::TableHeaderMissing,
            RuleId::TableHeaderEmpty,
            RuleId::HtmlUnchecked,
            RuleId::MediaMissing,
            RuleId::FeaturedAltMissing,
            RuleId::LinkInternalBroken,
            RuleId::LinkInternalRedirected,
            RuleId::LinkAnchorMissing,
            RuleId::LinkExternalBroken,
            RuleId::Orphan,
            RuleId::RedirectChain,
            RuleId::RedirectLoop,
            RuleId::RedirectTargetBroken,
            RuleId::Stale,
            RuleId::CheckTruncated,
            RuleId::FindingsTruncated,
        ]
        .iter()
        .map(|r| {
            serde_json::to_value(r)
                .unwrap()
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
        assert_eq!(ours, names);
    }
}

/// One health run.
#[derive(Debug, Serialize, ToSchema)]
pub struct HealthRunDto {
    pub id: Uuid,
    /// `running`, `done`, `failed` or `abandoned`.
    pub status: String,
    /// `schedule` or `manual`.
    pub trigger: String,
    pub requested_by: Option<Uuid>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub items_checked: i64,
    pub paths_checked: i64,
    pub urls_checked: i64,
    /// Unsuppressed findings: `{"rules": {"<rule>": n}, "severity": {"error": n, …}}`.
    #[schema(value_type = Object)]
    pub totals: Value,
    pub truncated: bool,
}

impl From<&ferrite_domain::HealthRun> for HealthRunDto {
    fn from(r: &ferrite_domain::HealthRun) -> Self {
        Self {
            id: r.id.as_uuid(),
            status: r.status.as_str().to_owned(),
            trigger: r.trigger.as_str().to_owned(),
            requested_by: r.requested_by.map(|u| u.as_uuid()),
            started_at: r.started_at.as_datetime(),
            finished_at: r.finished_at.map(|t| t.as_datetime()),
            items_checked: r.counters.items,
            paths_checked: r.counters.paths,
            urls_checked: r.counters.urls,
            totals: r.totals.clone(),
            truncated: r.truncated,
        }
    }
}

/// `GET /health/summary`.
#[derive(Debug, Serialize, ToSchema)]
pub struct HealthSummaryDto {
    /// The newest finished run.
    pub last: Option<HealthRunDto>,
    /// The run going on now.
    pub running: Option<HealthRunDto>,
    /// The next daily run (04:00 UTC).
    pub next_at: DateTime<Utc>,
    /// Whether external links are checked (`health.external_links`).
    pub external_links: bool,
}

/// The item a health finding is on.
#[derive(Debug, Serialize, ToSchema)]
pub struct HealthItemRefDto {
    pub id: Uuid,
    #[serde(rename = "type")]
    pub kind: String,
    pub title: String,
    pub status: String,
    pub locale: String,
    /// The item's editor in the admin.
    pub admin_url: String,
}

/// The redirect a health finding is on.
#[derive(Debug, Serialize, ToSchema)]
pub struct RedirectRefDto {
    pub id: Uuid,
    pub source: String,
    pub target: String,
}

/// One finding of the health report.
#[derive(Debug, Serialize, ToSchema)]
pub struct HealthFindingDto {
    #[schema(value_type = RuleId)]
    pub rule: String,
    #[schema(value_type = SeverityDto)]
    pub severity: String,
    #[schema(value_type = CategoryDto)]
    pub category: String,
    pub wcag: Vec<&'static str>,
    pub key: String,
    pub context: Option<String>,
    /// Rule-specific facts; `location` says where in the item.
    #[schema(value_type = Object)]
    pub detail: Value,
    pub item: Option<HealthItemRefDto>,
    pub redirect: Option<RedirectRefDto>,
    pub suppressed: Option<SuppressionDto>,
}

impl From<&ferrite_core::quality::HealthFinding> for HealthFindingDto {
    fn from(h: &ferrite_core::quality::HealthFinding) -> Self {
        let f = &h.stored.finding;
        Self {
            wcag: ferrite_core::quality::rule(&f.rule)
                .map(|r| r.wcag.to_vec())
                .unwrap_or_default(),
            rule: f.rule.clone(),
            severity: f.severity.clone(),
            category: f.category.clone(),
            key: f.key.clone(),
            context: f.context.clone(),
            detail: f.detail.clone(),
            item: h.item.as_ref().map(|i| HealthItemRefDto {
                id: i.id.as_uuid(),
                admin_url: format!("/admin/content/{}/{}", i.kind, i.id),
                kind: i.kind.clone(),
                title: i.title.clone(),
                status: i.status.as_str().to_owned(),
                locale: i.locale.clone(),
            }),
            redirect: h.redirect.as_ref().map(|r| RedirectRefDto {
                id: r.id.as_uuid(),
                source: r.source_path.clone(),
                target: r.target.clone(),
            }),
            suppressed: h.suppressed.as_ref().map(Into::into),
        }
    }
}

/// `GET /health/findings`.
#[derive(Debug, Serialize, ToSchema)]
pub struct HealthFindingList {
    pub items: Vec<HealthFindingDto>,
    pub next_cursor: Option<String>,
}

/// `POST /health/runs`: the queued job.
#[derive(Debug, Serialize, ToSchema)]
pub struct HealthRunQueuedDto {
    pub job_id: Uuid,
}
