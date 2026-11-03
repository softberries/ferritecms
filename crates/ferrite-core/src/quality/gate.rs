//! The per-type publish gate (design 16 §5, ruling P4).
//!
//! [`check`] is the gate: `ContentService::publish` and `schedule` call it inside the
//! transition's transaction, and other publishing paths that need a gate (M6-3's
//! releases) call it themselves for every item they publish, inside their own
//! transaction, treating any `Err` as "the whole release is refused" (all-or-nothing,
//! P4). It reads only the item, its type and its suppressions: no router, no network,
//! no media (only `accessibility` errors gate, D4), so it is deterministic from the
//! item.

use ferrite_domain::{ContentId, ContentItem, ContentType, QualityGate, Suppression};
use ferrite_store::Repos;
use serde_json::Value;

use super::{Category, DocInput, Finding, Severity, check_document};
use crate::{CoreError, fields};

/// Most findings a [`CoreError::QualityBlocked`] lists.
pub const MAX_BLOCKING_LISTED: usize = 50;

/// Whether `s` covers `f`: the same rule, and the same key or the whole rule (`""`).
pub fn covers(s: &Suppression, f: &Finding) -> bool {
    s.rule == f.rule.id && (s.key.is_empty() || s.key == f.key)
}

/// The suppression of `f` among `suppressions`, if any.
pub fn suppression_of<'a>(suppressions: &'a [Suppression], f: &Finding) -> Option<&'a Suppression> {
    suppressions.iter().find(|s| covers(s, f))
}

/// Whether `f` is one the gate counts: an `accessibility` error.
pub fn blocks(f: &Finding) -> bool {
    f.rule.category == Category::Accessibility && f.rule.severity == Severity::Error
}

/// The unsuppressed accessibility errors of `title`, `body` and `fields` of an item of
/// type `ty` in `locale` (`item`: whose suppressions apply; `None` for a draft).
pub async fn blocking(
    repos: &dyn Repos,
    doc: (&str, &Value, &Value),
    ty: &ContentType,
    locale: &str,
    item: Option<ContentId>,
) -> Result<Vec<Finding>, CoreError> {
    let (title, body, fields_value) = doc;
    let defs = fields::schema_of(&ty.fields)
        .ok()
        .flatten()
        .unwrap_or_default();
    let (findings, _, _) = check_document(
        &DocInput {
            title,
            body,
            fields: fields_value,
            field_defs: &defs,
            locale,
        },
        "",
    );
    let suppressions = match item {
        Some(id) => repos.quality().suppressions_for(&[id]).await?,
        None => Vec::new(),
    };
    Ok(findings
        .into_iter()
        .filter(|f| blocks(f) && suppression_of(&suppressions, f).is_none())
        .collect())
}

/// The number of unsuppressed accessibility errors (see [`blocking`]).
pub async fn error_count(
    repos: &dyn Repos,
    doc: (&str, &Value, &Value),
    ty: &ContentType,
    locale: &str,
    item: Option<ContentId>,
) -> Result<usize, CoreError> {
    Ok(blocking(repos, doc, ty, locale, item).await?.len())
}

/// The publish gate for `item` of type `ty`, read in the caller's transaction: `Ok` when
/// the type's gate is `off` or the item has no unsuppressed accessibility error, else
/// [`CoreError::QualityBlocked`] with at most [`MAX_BLOCKING_LISTED`] findings.
///
/// Callers that publish several items at once (M6-3 releases) call it for each and
/// refuse the whole batch on the first `Err` (ruling P4).
pub async fn check(
    repos: &dyn Repos,
    item: &ContentItem,
    ty: &ContentType,
) -> Result<(), CoreError> {
    if ty.quality_gate != QualityGate::Errors {
        return Ok(());
    }
    let mut found = blocking(
        repos,
        (&item.title, &item.body, &item.fields),
        ty,
        &item.locale,
        Some(item.id),
    )
    .await?;
    if found.is_empty() {
        return Ok(());
    }
    found.truncate(MAX_BLOCKING_LISTED);
    Err(CoreError::QualityBlocked(Box::new(found)))
}
