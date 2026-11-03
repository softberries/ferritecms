//! [`QualityService`]: the editor's and the assistants' checks, and suppressions
//! (design 16 §7.2, §10). Capabilities are checked here, never in handlers.

use std::{collections::HashMap, sync::Arc};

use ferrite_domain::{
    ContentId, ContentItem, ContentType, NewAudit, QualityGate, Suppression, Timestamp,
};
use ferrite_store::{Repos, Store};
use serde_json::{Value, json};

use super::{
    Finding, FoundLink, MAX_ITEM_FINDINGS, MAX_KEY, PathProbe, PathStatus, RuleScope, Severity,
    check_document, cut, facts,
    gate::{blocks, suppression_of},
    rule,
    rules::known,
};
use crate::{
    Actor, CoreError,
    actor::{Area, Capability, can_edit, can_read, require, require_read},
    fields,
    tx::finish,
};

/// Most distinct internal paths one check probes (design 16 §6.2).
pub const MAX_CHECK_PATHS: usize = 200;
/// Longest suppression reason (characters).
pub const MAX_REASON: usize = 500;

/// The editor's (or an assistant's) working copy to check.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckDraft {
    /// The stored item it is a draft of: its suppressions and locale apply.
    pub id: Option<ContentId>,
    pub title: String,
    pub body: Value,
    pub fields: Value,
    /// For a new item (an existing item's own locale wins); `None`: the site's default.
    pub locale: Option<String>,
}

/// A finding and, when a person accepted it, the suppression.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckedFinding {
    pub finding: Finding,
    pub suppressed: Option<Suppression>,
}

/// Unsuppressed findings per severity, and the suppressed ones.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub error: u32,
    pub warning: u32,
    pub notice: u32,
    pub suppressed: u32,
}

impl Counts {
    pub(crate) fn add(&mut self, f: &Finding, suppressed: bool) {
        if suppressed {
            self.suppressed += 1;
            return;
        }
        match f.rule.severity {
            Severity::Error => self.error += 1,
            Severity::Warning => self.warning += 1,
            Severity::Notice => self.notice += 1,
        }
    }
}

/// The type's gate and whether it would refuse this draft now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GateState {
    pub mode: QualityGate,
    pub blocked: bool,
}

/// What [`QualityService::check`] found.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckReport {
    pub findings: Vec<CheckedFinding>,
    pub counts: Counts,
    pub gate: GateState,
}

/// Checks drafts and items, and keeps suppressions.
pub struct QualityService {
    pub(super) store: Arc<dyn Store>,
    site_origin: String,
}

impl QualityService {
    /// `site_url`: the site's public URL; links to its origin are internal.
    pub fn new(store: Arc<dyn Store>, site_url: String) -> Self {
        Self {
            store,
            site_origin: site_url,
        }
    }

    /// The site URL links are classified against.
    pub fn site_url(&self) -> &str {
        &self.site_origin
    }

    /// Runs the item-scope rules over `draft` of type `kind` (needs `content:read`; for
    /// `draft.id`, read access to that item, of that type, else `NotFound`): the
    /// document checks, the media facts, and with a `probe` at most
    /// [`MAX_CHECK_PATHS`] distinct internal paths. Suppressions of the item are marked
    /// and count nowhere.
    pub async fn check(
        &self,
        actor: &Actor,
        kind: &str,
        draft: CheckDraft,
        probe: Option<&dyn PathProbe>,
    ) -> Result<CheckReport, CoreError> {
        require_read(actor, Area::Content)?;
        let repos = self.store.read();
        let ty = repos.types().get(kind).await?.ok_or(CoreError::NotFound)?;
        let (item, suppressions) = match draft.id {
            Some(id) => {
                let item = readable(repos, actor, id).await?;
                if item.kind != kind {
                    return Err(CoreError::NotFound);
                }
                let s = repos.quality().suppressions_for(&[id]).await?;
                (Some(item), s)
            }
            None => (None, Vec::new()),
        };
        let locale = match (&item, draft.locale) {
            (Some(i), _) => i.locale.clone(),
            (None, Some(l)) => l,
            (None, None) => crate::locales::read(repos).await?.default.tag.clone(),
        };
        let findings = self
            .findings_of(
                repos,
                &ty,
                (&draft.title, &draft.body, &draft.fields),
                &locale,
                probe,
            )
            .await?;
        Ok(report(findings, &suppressions, ty.quality_gate))
    }

    /// The item-scope findings of one document, with media facts and (with a probe) the
    /// internal links.
    pub(crate) async fn findings_of(
        &self,
        repos: &dyn Repos,
        ty: &ContentType,
        doc: (&str, &Value, &Value),
        locale: &str,
        probe: Option<&dyn PathProbe>,
    ) -> Result<Vec<Finding>, CoreError> {
        let defs = fields::schema_of(&ty.fields)
            .ok()
            .flatten()
            .unwrap_or_default();
        let (title, body, fields_value) = doc;
        let (mut findings, links, media) = check_document(
            &super::DocInput {
                title,
                body,
                fields: fields_value,
                field_defs: &defs,
                locale,
            },
            &self.site_origin,
        );
        let media_facts = facts::load(repos, &media).await?;
        facts::apply(&media_facts, &mut findings, &media);
        if let Some(probe) = probe {
            let mut statuses: HashMap<String, PathStatus> = HashMap::new();
            for link in &links.internal {
                if !statuses.contains_key(&link.target) {
                    if statuses.len() >= MAX_CHECK_PATHS {
                        continue;
                    }
                    let status = probe.probe(&link.target).await;
                    statuses.insert(link.target.clone(), status);
                }
            }
            for link in &links.internal {
                if findings.len() >= MAX_ITEM_FINDINGS {
                    break;
                }
                if let Some(f) = statuses
                    .get(&link.target)
                    .and_then(|s| link_finding(link, s))
                {
                    findings.push(f);
                }
            }
        }
        Ok(findings)
    }

    /// The suppressions of item `id` (read access to it).
    pub async fn suppressions(
        &self,
        actor: &Actor,
        id: ContentId,
    ) -> Result<Vec<Suppression>, CoreError> {
        require_read(actor, Area::Content)?;
        let repos = self.store.read();
        readable(repos, actor, id).await?;
        Ok(repos.quality().suppressions_for(&[id]).await?)
    }

    /// Suppresses `rule` (`key`: one finding, `""`: the whole rule) on item `id` with a
    /// `reason` (1..=500 characters, no control characters). Needs the right to edit
    /// the item; an `error` on an item of a gated type needs `edit_others` (D15). An
    /// unknown rule, or one whose findings never belong to an item, is
    /// `InvalidField { field: "rule" }`. Audited as `quality.suppress`.
    pub async fn suppress(
        &self,
        actor: &Actor,
        id: ContentId,
        rule_id: &str,
        key: &str,
        reason: &str,
    ) -> Result<Suppression, CoreError> {
        let reason = reason.trim();
        if reason.is_empty()
            || reason.chars().count() > MAX_REASON
            || reason.chars().any(char::is_control)
        {
            return Err(CoreError::invalid_field(
                "reason",
                format!("a reason of 1 to {MAX_REASON} characters without control characters"),
            ));
        }
        check_key(key)?;
        let tx = self.store.begin().await?;
        let done = async {
            let (item, r) = self
                .suppress_rights(tx.as_ref(), actor, id, rule_id)
                .await?;
            let s = Suppression {
                item_id: item.id,
                rule: r.id.to_owned(),
                key: key.to_owned(),
                reason: reason.to_owned(),
                user_id: Some(actor.user),
                created_at: Timestamp::now(),
            };
            tx.quality().suppress(&s).await?;
            audit(tx.as_ref(), actor, "quality.suppress", id, r.id, key).await?;
            Ok(s)
        }
        .await;
        finish(tx, done).await
    }

    /// Removes a suppression (the same rights as [`suppress`](Self::suppress)); a
    /// missing one is `NotFound`. Audited as `quality.unsuppress`.
    pub async fn unsuppress(
        &self,
        actor: &Actor,
        id: ContentId,
        rule_id: &str,
        key: &str,
    ) -> Result<(), CoreError> {
        check_key(key)?;
        let tx = self.store.begin().await?;
        let done = async {
            let (_, r) = self
                .suppress_rights(tx.as_ref(), actor, id, rule_id)
                .await?;
            if !tx.quality().unsuppress(id, r.id, key).await? {
                return Err(CoreError::NotFound);
            }
            audit(tx.as_ref(), actor, "quality.unsuppress", id, r.id, key).await
        }
        .await;
        finish(tx, done).await
    }

    async fn suppress_rights(
        &self,
        repos: &dyn Repos,
        actor: &Actor,
        id: ContentId,
        rule_id: &str,
    ) -> Result<(ContentItem, &'static super::Rule), CoreError> {
        let item = readable(repos, actor, id).await?;
        can_edit(actor, &item)?;
        let r = rule(rule_id).filter(|r| suppressible(r)).ok_or_else(|| {
            CoreError::invalid_field("rule", "not a rule a finding on an item has")
        })?;
        if r.severity == Severity::Error {
            let gated = repos
                .types()
                .get(&item.kind)
                .await?
                .is_some_and(|t| t.quality_gate == QualityGate::Errors);
            if gated {
                require(actor, Capability::EditOthers)?;
            }
        }
        Ok((item, r))
    }
}

/// Whether findings of `r` can be suppressed: every rule whose findings belong to an
/// item (the item-scope rules, orphan, stale, external links); not the redirect rules
/// (fixed in Redirects) nor the engine's truncation notes.
fn suppressible(r: &super::Rule) -> bool {
    !(r.id.starts_with("redirect-") || r.id.ends_with("-truncated"))
        && (r.scope == RuleScope::Item
            || matches!(r.id, "orphan" | "stale" | "link-external-broken"))
}

fn check_key(key: &str) -> Result<(), CoreError> {
    if key.chars().count() > MAX_KEY || key.chars().any(char::is_control) {
        return Err(CoreError::invalid_field(
            "key",
            format!("a key of at most {MAX_KEY} characters without control characters"),
        ));
    }
    Ok(())
}

async fn audit(
    repos: &dyn Repos,
    actor: &Actor,
    action: &str,
    id: ContentId,
    rule_id: &str,
    key: &str,
) -> Result<(), CoreError> {
    repos
        .audit()
        .append(&NewAudit {
            actor_id: Some(actor.user),
            action: action.into(),
            target_kind: "content".into(),
            target_id: Some(id.to_string()),
            details: json!({ "rule": rule_id, "key": cut(key, MAX_KEY) }),
        })
        .await?;
    Ok(())
}

/// Item `id` if `actor` may read it, else `NotFound`.
async fn readable(
    repos: &dyn Repos,
    actor: &Actor,
    id: ContentId,
) -> Result<ContentItem, CoreError> {
    let item = repos.content().get(id).await?.ok_or(CoreError::NotFound)?;
    if !can_read(actor, &item, Timestamp::now()) {
        return Err(CoreError::NotFound);
    }
    Ok(item)
}

/// The finding of a probed internal link, if it needs one.
pub(crate) fn link_finding(link: &FoundLink, status: &PathStatus) -> Option<Finding> {
    let (rule_id, detail) = match status {
        PathStatus::NotFound => (
            "link-internal-broken",
            json!({"href": link.href, "status": 404}),
        ),
        PathStatus::Gone => (
            "link-internal-broken",
            json!({"href": link.href, "status": 410}),
        ),
        PathStatus::Redirected { to, .. } => (
            "link-internal-redirected",
            json!({"href": link.href, "to": cut(to, MAX_KEY)}),
        ),
        PathStatus::Ok { .. } | PathStatus::External(_) | PathStatus::Unchecked => return None,
    };
    Some(Finding {
        rule: known(rule_id),
        location: link.location.clone(),
        key: link.key(),
        context: Some(cut(&link.text, super::MAX_CONTEXT)),
        detail,
    })
}

/// Marks suppressed findings, counts the rest and works out the gate.
pub(crate) fn report(
    findings: Vec<Finding>,
    suppressions: &[Suppression],
    mode: QualityGate,
) -> CheckReport {
    let mut counts = Counts::default();
    let mut blocked = false;
    let findings = findings
        .into_iter()
        .map(|finding| {
            let suppressed = suppression_of(suppressions, &finding).cloned();
            counts.add(&finding, suppressed.is_some());
            if suppressed.is_none() && blocks(&finding) {
                blocked = true;
            }
            CheckedFinding {
                finding,
                suppressed,
            }
        })
        .collect();
    CheckReport {
        findings,
        counts,
        gate: GateState {
            mode,
            blocked: blocked && mode == QualityGate::Errors,
        },
    }
}
