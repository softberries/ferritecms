//! Redirect lookup and management.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use ferrite_domain::{
    MAX_PAGE, Page, REDIRECT_CODES, Redirect, RedirectId, RedirectOrigin, Timestamp,
};
use ferrite_store::{Repos, Store};

use crate::{
    Actor, CoreError, RowError,
    actor::{Capability, require},
    events,
    tx::finish,
};

/// Redirects followed when checking a new one for loops and long chains.
const MAX_HOPS: usize = 10;
/// Longest source or target accepted.
const MAX_URL_LEN: usize = 2048;
/// Most rows one [`RedirectService::import`] takes.
pub const MAX_IMPORT_ROWS: usize = 10_000;

/// What an import did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportReport {
    /// Rows whose source had no redirect.
    pub created: u32,
    /// Rows that replaced the target and code of an existing redirect.
    pub updated: u32,
}

/// Resolves and manages redirects (writes need `manage_redirects`).
///
/// **Rules** (every write path: [`upsert`](Self::upsert), [`update`](Self::update),
/// [`import`](Self::import)): `source` is a path starting with `/`, `target` a path or an
/// absolute http(s) URL, both at most 2048 characters without whitespace or control
/// characters (`InvalidField` `source` / `target`); `code` one of 301, 302, 307, 308,
/// 410 (`InvalidField` `code`). Rejected as `InvalidField { field: "target", .. }`:
/// `source == target`; a target that leads back to `source` through existing redirects
/// (loop); a target that starts a chain of more than 10 existing redirects. Hops are
/// followed the way [`resolve`](Self::resolve) matches them (exact path, then trailing
/// slash toggled), and `source` counts together with its toggled variant. The chain
/// limit counts downstream hops only; redirects that already lead into `source` are not
/// counted.
///
/// Absolute targets let a redirect send visitors to any site (an open redirect), which
/// is why this is admin-only (`manage_redirects`). Under the default isolation
/// (READ COMMITTED on Postgres) the checks are not serialized: two concurrent writes can
/// still close a loop together (plan ruling P2).
pub struct RedirectService {
    store: Arc<dyn Store>,
}

impl RedirectService {
    pub fn new(store: Arc<dyn Store>) -> Self {
        Self { store }
    }

    /// The redirect for `path`: the exact path first, then the path with its trailing
    /// slash toggled (`/a` and `/a/`; `/` has no variant). Matching is case-sensitive.
    pub async fn resolve(&self, path: &str) -> Result<Option<Redirect>, CoreError> {
        lookup(self.store.read(), path).await
    }

    /// Records a hit. Best effort: failures are logged and swallowed.
    pub async fn hit(&self, id: RedirectId) {
        if let Err(e) = self.store.read().redirects().hit(id).await {
            tracing::warn!(redirect = %id, error = %e, "redirect hit not recorded");
        }
    }

    /// Lists redirects with cursor pagination.
    pub async fn list(
        &self,
        actor: &Actor,
        limit: u32,
        cursor: Option<&str>,
    ) -> Result<Page<Redirect>, CoreError> {
        require(actor, Capability::ManageRedirects)?;
        Ok(self.store.read().redirects().list(limit, cursor).await?)
    }

    /// Redirect `id` (`manage_redirects`; missing: `NotFound`).
    pub async fn get(&self, actor: &Actor, id: RedirectId) -> Result<Redirect, CoreError> {
        require(actor, Capability::ManageRedirects)?;
        self.store
            .read()
            .redirects()
            .get(id)
            .await?
            .ok_or(CoreError::NotFound)
    }

    /// Creates a manual redirect from `source` to `target`, or changes the target and
    /// code of the existing one for `source` (see the type docs for the rules).
    pub async fn upsert(
        &self,
        actor: &Actor,
        source: &str,
        target: &str,
        code: u16,
    ) -> Result<Redirect, CoreError> {
        require(actor, Capability::ManageRedirects)?;
        check_row(source, target, code)?;
        let tx = self.store.begin().await?;
        let done = async {
            check_chain(tx.as_ref(), source, target).await?;
            let saved = tx
                .redirects()
                .upsert(&Redirect {
                    id: RedirectId::new(),
                    source_path: source.to_owned(),
                    target: target.to_owned(),
                    code,
                    hits: 0,
                    origin: RedirectOrigin::Manual,
                    created_at: Timestamp::now(),
                    last_hit_at: None,
                })
                .await?;
            emit_redirects_changed(tx.as_ref(), [source.to_owned()]).await?;
            Ok(saved)
        }
        .await;
        finish(tx, done).await
    }

    /// Changes redirect `id` (missing: `NotFound`) to `source` → `target` with `code`,
    /// as a manual redirect (see the type docs for the rules). A new `source` that
    /// another redirect holds, exactly or with its trailing slash toggled, is
    /// `InvalidField { field: "source", .. }` ("source taken", F24). The row keeps its
    /// id, hits and times: when the source changes, the old row is deleted and the new
    /// one written with the same id in one transaction, which emits the keys of both
    /// sources.
    pub async fn update(
        &self,
        actor: &Actor,
        id: RedirectId,
        source: &str,
        target: &str,
        code: u16,
    ) -> Result<Redirect, CoreError> {
        require(actor, Capability::ManageRedirects)?;
        check_row(source, target, code)?;
        let tx = self.store.begin().await?;
        let done = async {
            let repos = tx.as_ref();
            let current = repos
                .redirects()
                .get(id)
                .await?
                .ok_or(CoreError::NotFound)?;
            if current.source_path != source {
                let mut variants = vec![source.to_owned()];
                variants.extend(toggled(source));
                for v in &variants {
                    if repos
                        .redirects()
                        .get_by_source(v)
                        .await?
                        .is_some_and(|r| r.id != id)
                    {
                        return Err(CoreError::invalid_field(
                            "source",
                            "source taken by another redirect",
                        ));
                    }
                }
                // The old mapping must not take part in the loop check.
                repos.redirects().delete(id).await?;
            }
            check_chain(repos, source, target).await?;
            let saved = repos
                .redirects()
                .upsert(&Redirect {
                    source_path: source.to_owned(),
                    target: target.to_owned(),
                    code,
                    origin: RedirectOrigin::Manual,
                    ..current.clone()
                })
                .await?;
            if saved.id != id {
                // A concurrent write took the source after the check above: the upsert
                // hit that row. Roll back rather than change it.
                return Err(CoreError::invalid_field(
                    "source",
                    "source taken by another redirect",
                ));
            }
            let mut sources = vec![current.source_path];
            if sources[0] != source {
                sources.push(source.to_owned());
            }
            emit_redirects_changed(repos, sources).await?;
            Ok(saved)
        }
        .await;
        finish(tx, done).await
    }

    /// Imports `rows` of `(source, target, code)` with origin `import` (F25): each row
    /// creates the redirect for its source or replaces the target and code of the
    /// existing one. At most [`MAX_IMPORT_ROWS`] rows (else `Invalid`).
    ///
    /// Every row is checked first with the rules of the type docs, against the existing
    /// redirects (read before the transaction) with all of the file's rows applied on
    /// top, so loops and long chains among the file's own rows count too; a source
    /// given twice is an error on its second row. If any row fails nothing is written
    /// and the result is `InvalidRows` listing each failing row. Otherwise one
    /// transaction upserts every row and emits one `invalidate` with the keys of every
    /// source. A redirect changed between the read and the write can still close a loop
    /// (P2).
    pub async fn import(
        &self,
        actor: &Actor,
        rows: &[(String, String, u16)],
    ) -> Result<ImportReport, CoreError> {
        require(actor, Capability::ManageRedirects)?;
        if rows.len() > MAX_IMPORT_ROWS {
            return Err(CoreError::Invalid(format!(
                "an import takes at most {MAX_IMPORT_ROWS} rows"
            )));
        }
        let mut errors = Vec::new();
        let mut seen: HashSet<&str> = HashSet::new();
        let mut valid = vec![false; rows.len()];
        for (i, (source, target, code)) in rows.iter().enumerate() {
            let checked = check_row(source, target, *code).and_then(|()| {
                if seen.insert(source.as_str()) {
                    Ok(())
                } else {
                    Err(CoreError::invalid_field(
                        "source",
                        format!("duplicate source {source:?} (given on an earlier row)"),
                    ))
                }
            });
            match checked {
                Ok(()) => valid[i] = true,
                Err(e) => errors.push(row_error(i, e)),
            }
        }
        // Existing redirects with the file on top.
        let mut map = self.all_targets().await?;
        for (i, (source, target, _)) in rows.iter().enumerate() {
            if valid[i] {
                map.insert(source.clone(), target.clone());
            }
        }
        for (i, (source, target, _)) in rows.iter().enumerate() {
            if valid[i]
                && let Err(e) = check_chain_in(&map, source, target)
            {
                errors.push(row_error(i, e));
            }
        }
        if !errors.is_empty() {
            errors.sort_by_key(|e| e.row);
            return Err(CoreError::InvalidRows(errors));
        }
        if rows.is_empty() {
            return Ok(ImportReport {
                created: 0,
                updated: 0,
            });
        }
        let tx = self.store.begin().await?;
        let done = async {
            let mut report = ImportReport {
                created: 0,
                updated: 0,
            };
            let now = Timestamp::now();
            for (source, target, code) in rows {
                let id = RedirectId::new();
                let saved = tx
                    .redirects()
                    .upsert(&Redirect {
                        id,
                        source_path: source.clone(),
                        target: target.clone(),
                        code: *code,
                        hits: 0,
                        origin: RedirectOrigin::Import,
                        created_at: now,
                        last_hit_at: None,
                    })
                    .await?;
                if saved.id == id {
                    report.created += 1;
                } else {
                    report.updated += 1;
                }
            }
            emit_redirects_changed(tx.as_ref(), rows.iter().map(|(s, _, _)| s.clone())).await?;
            Ok(report)
        }
        .await;
        finish(tx, done).await
    }

    /// Deletes a redirect (a missing one is a no-op).
    pub async fn delete(&self, actor: &Actor, id: RedirectId) -> Result<(), CoreError> {
        require(actor, Capability::ManageRedirects)?;
        let tx = self.store.begin().await?;
        let done = async {
            if let Some(r) = tx.redirects().get(id).await? {
                tx.redirects().delete(id).await?;
                emit_redirects_changed(tx.as_ref(), [r.source_path]).await?;
            }
            Ok(())
        }
        .await;
        finish(tx, done).await
    }

    /// Every stored redirect as source → target (read pool, page by page).
    async fn all_targets(&self) -> Result<HashMap<String, String>, CoreError> {
        let repo = self.store.read().redirects();
        let mut map = HashMap::new();
        let mut cursor: Option<String> = None;
        loop {
            let page = repo.list(MAX_PAGE, cursor.as_deref()).await?;
            map.extend(page.items.into_iter().map(|r| (r.source_path, r.target)));
            match page.next_cursor {
                Some(c) => cursor = Some(c),
                None => return Ok(map),
            }
        }
    }
}

fn row_error(row: usize, e: CoreError) -> RowError {
    let message = match e {
        CoreError::Invalid(m) | CoreError::InvalidField { message: m, .. } => m,
        other => other.to_string(),
    };
    RowError { row, message }
}

/// The checks of one row that need no store: source, target, code, not to itself.
fn check_row(source: &str, target: &str, code: u16) -> Result<(), CoreError> {
    check_source(source)?;
    check_target(target)?;
    if !REDIRECT_CODES.contains(&code) {
        return Err(CoreError::invalid_field(
            "code",
            "redirect code must be 301, 302, 307, 308 or 410",
        ));
    }
    if source == target || toggled(source).as_deref() == Some(target) {
        return Err(CoreError::invalid_field("target", "redirect to itself"));
    }
    Ok(())
}

/// The loop and chain rules for `source` → `target` against the redirects `repos` holds.
async fn check_chain(repos: &dyn Repos, source: &str, target: &str) -> Result<(), CoreError> {
    let ours = variants(source);
    let mut at = target.to_owned();
    for _ in 0..MAX_HOPS {
        let Some(next) = lookup(repos, &at).await? else {
            return Ok(());
        };
        if ours.contains(&next.target) {
            return Err(loop_error());
        }
        at = next.target;
    }
    if lookup(repos, &at).await?.is_some() {
        return Err(chain_error());
    }
    Ok(())
}

/// [`check_chain`] against an in-memory source → target map.
fn check_chain_in(
    map: &HashMap<String, String>,
    source: &str,
    target: &str,
) -> Result<(), CoreError> {
    let find = |path: &str| -> Option<&String> {
        map.get(path)
            .or_else(|| toggled(path).and_then(|other| map.get(&other)))
    };
    let ours = variants(source);
    let mut at = target.to_owned();
    for _ in 0..MAX_HOPS {
        let Some(next) = find(&at) else {
            return Ok(());
        };
        if ours.contains(next) {
            return Err(loop_error());
        }
        at = next.clone();
    }
    if find(&at).is_some() {
        return Err(chain_error());
    }
    Ok(())
}

/// `source` and its slash-toggled variant.
fn variants(source: &str) -> Vec<String> {
    let mut v = vec![source.to_owned()];
    v.extend(toggled(source));
    v
}

fn loop_error() -> CoreError {
    CoreError::invalid_field("target", "redirect loop")
}

fn chain_error() -> CoreError {
    CoreError::invalid_field("target", format!("redirect chain longer than {MAX_HOPS}"))
}

/// Appends one `invalidate` event with `redirect:<source>` for each source and its
/// toggled variant; nothing for an empty list. Informational (see [`events`]).
pub(crate) async fn emit_redirects_changed(
    repos: &dyn Repos,
    sources: impl IntoIterator<Item = String>,
) -> Result<(), CoreError> {
    let keys: Vec<String> = sources
        .into_iter()
        .flat_map(|s| {
            let variant = toggled(&s);
            std::iter::once(s).chain(variant)
        })
        .map(|s| events::redirect_key(&s))
        .collect();
    if !keys.is_empty() {
        repos.events().append(&events::invalidate(keys)).await?;
    }
    Ok(())
}

/// The redirect for `path`: exact match first, then the slash-toggled path.
async fn lookup(repos: &dyn Repos, path: &str) -> Result<Option<Redirect>, CoreError> {
    if let Some(r) = repos.redirects().get_by_source(path).await? {
        return Ok(Some(r));
    }
    match toggled(path) {
        Some(other) => Ok(repos.redirects().get_by_source(&other).await?),
        None => Ok(None),
    }
}

/// `/a` <-> `/a/`; `None` for `/` and for paths without a leading `/`.
pub(crate) fn toggled(path: &str) -> Option<String> {
    if path == "/" || !path.starts_with('/') {
        return None;
    }
    Some(match path.strip_suffix('/') {
        Some(stripped) => stripped.to_owned(),
        None => format!("{path}/"),
    })
}

fn check_source(source: &str) -> Result<(), CoreError> {
    if source.chars().count() > MAX_URL_LEN {
        return Err(CoreError::invalid_field(
            "source",
            format!("redirect source longer than {MAX_URL_LEN} characters"),
        ));
    }
    if !source.starts_with('/') || source.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(CoreError::invalid_field(
            "source",
            "redirect source must be a path starting with '/'",
        ));
    }
    Ok(())
}

fn check_target(target: &str) -> Result<(), CoreError> {
    if target.chars().count() > MAX_URL_LEN {
        return Err(CoreError::invalid_field(
            "target",
            format!("redirect target longer than {MAX_URL_LEN} characters"),
        ));
    }
    let lower = target.to_ascii_lowercase();
    let ok_scheme = ["http://", "https://"]
        .iter()
        .any(|p| lower.starts_with(p) && lower.len() > p.len());
    let ok_path = target.starts_with('/') && !target.starts_with("//");
    if !(ok_scheme || ok_path)
        || target.chars().any(|c| c.is_control() || c.is_whitespace())
        || target.contains('\\')
    {
        return Err(CoreError::invalid_field(
            "target",
            "redirect target must be a path or an http(s) URL",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_variants() {
        assert_eq!(toggled("/a").as_deref(), Some("/a/"));
        assert_eq!(toggled("/a/").as_deref(), Some("/a"));
        assert_eq!(toggled("/"), None);
        assert_eq!(toggled("a"), None);
    }
}
