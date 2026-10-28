//! Redirect lookup and management.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use ferrite_domain::{
    MAX_PAGE, Page, REDIRECT_CODES, Redirect, RedirectId, RedirectOrigin, RedirectQuery, Timestamp,
};
use ferrite_store::{MAX_REDIRECTS_BY_TARGET, Repos, Store};

use crate::{
    Actor, CoreError, RowError,
    actor::{Area, Capability, can, require, require_read},
    events, slug,
    tx::finish,
};

/// Redirects followed when checking a new one for loops and long chains.
const MAX_HOPS: usize = 10;
/// Longest source or target accepted.
const MAX_URL_LEN: usize = 2048;
/// Longest search text [`RedirectService::list`] takes, in characters (after trimming).
pub const MAX_REDIRECT_QUERY: usize = 200;
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

/// Resolves and manages redirects. Writes need `manage_redirects` (scope
/// `redirects:write`); reads ([`list`](Self::list), [`get`](Self::get)) need a role
/// holding `manage_redirects` and the scope `redirects:read` (or write).
///
/// **Rules** (every write path: [`upsert`](Self::upsert), [`update`](Self::update),
/// [`import`](Self::import)): `source` is a path starting with `/`, `target` a path or an
/// absolute http(s) URL, both at most 2048 characters without whitespace or control
/// characters (`InvalidField` `source` / `target`); a source whose first segment is
/// router-owned is refused too (`check_source`); `code` one of 301, 302, 307, 308,
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

    /// Adds `n` (>= 1) hits, the last one at `at`, in one statement
    /// ([`RedirectRepo::add_hits`](ferrite_store::RedirectRepo::add_hits): `last_hit_at`
    /// never moves back; a deleted redirect is `Ok`). No capability check: hits come
    /// from the public site, not from an actor.
    pub async fn add_hits(&self, id: RedirectId, n: u32, at: Timestamp) -> Result<(), CoreError> {
        Ok(self.store.read().redirects().add_hits(id, n, at).await?)
    }

    /// Lists redirects by source with cursor pagination. `query.q` is trimmed; blank is
    /// no search (as on the media list), longer than [`MAX_REDIRECT_QUERY`] characters
    /// is `InvalidField` `q`; it matches `source` or `target` as a case-insensitive
    /// substring, wildcards literal
    /// ([`RedirectRepo::list`](ferrite_store::RedirectRepo::list)).
    pub async fn list(
        &self,
        actor: &Actor,
        mut query: RedirectQuery,
    ) -> Result<Page<Redirect>, CoreError> {
        require_view(actor)?;
        if let Some(q) = query.q.take() {
            let q = q.trim();
            if q.chars().count() > MAX_REDIRECT_QUERY {
                return Err(CoreError::invalid_field(
                    "q",
                    format!("q must be at most {MAX_REDIRECT_QUERY} characters"),
                ));
            }
            query.q = (!q.is_empty()).then(|| q.to_owned());
        }
        Ok(self.store.read().redirects().list(&query).await?)
    }

    /// Redirect `id` (see the type docs for the read check; missing: `NotFound`).
    pub async fn get(&self, actor: &Actor, id: RedirectId) -> Result<Redirect, CoreError> {
        require_view(actor)?;
        self.store
            .read()
            .redirects()
            .get(id)
            .await?
            .ok_or(CoreError::NotFound)
    }

    /// Creates a manual redirect from `source` to `target`, or changes the target and
    /// code of the existing one for `source` (see the type docs for the rules). Creating
    /// a `source` whose trailing-slash variant another redirect holds is
    /// `InvalidField { field: "source", .. }` ("source taken", as [`update`](Self::update)
    /// counts it; R26), so the two variants cannot lead to different targets; a stored
    /// pair from before is still changed in place.
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
            // Creating `source` while its variant is stored would split the two; an
            // existing row for `source` itself (legacy pairs) is changed in place.
            if let Some(other) = toggled(source)
                && tx.redirects().get_by_source(&other).await?.is_some()
                && tx.redirects().get_by_source(source).await?.is_none()
            {
                return Err(source_taken());
            }
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
                        return Err(source_taken());
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
                return Err(source_taken());
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
    /// redirects (read in the write's transaction) with all of the file's rows applied on
    /// top, so loops and long chains among the file's own rows count too; a source
    /// given twice, exactly or with its trailing slash toggled (as
    /// [`update`](Self::update) counts it), is an error on its second row, and so is a
    /// new source whose toggled variant is already stored (R26). If any row fails nothing
    /// is written and the result is `InvalidRows` listing each failing row. Otherwise the same
    /// transaction upserts every row and emits one `invalidate` with the keys of every
    /// source. A concurrent write can still close a loop (P2).
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
        if rows.is_empty() {
            return Ok(ImportReport {
                created: 0,
                updated: 0,
            });
        }
        let tx = self.store.begin().await?;
        let done = async {
            let report = import_in(tx.as_ref(), rows).await?;
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
}

/// Every stored redirect as source → target, page by page.
pub(crate) async fn all_targets_in(
    repos: &dyn Repos,
) -> Result<HashMap<String, String>, CoreError> {
    let repo = repos.redirects();
    let mut map = HashMap::new();
    let mut query = RedirectQuery {
        q: None,
        limit: MAX_PAGE,
        cursor: None,
    };
    loop {
        let page = repo.list(&query).await?;
        map.extend(page.items.into_iter().map(|r| (r.source_path, r.target)));
        match page.next_cursor {
            Some(c) => query.cursor = Some(c),
            None => return Ok(map),
        }
    }
}

/// The row checks of [`RedirectService::import`] against `existing` (source → target of
/// the stored redirects) with all valid rows applied on top; the failing rows, by row.
pub(crate) fn validate_rows(
    rows: &[(String, String, u16)],
    mut map: HashMap<String, String>,
) -> Vec<RowError> {
    let mut errors = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    let mut valid = vec![false; rows.len()];
    for (i, (source, target, code)) in rows.iter().enumerate() {
        let checked = check_row(source, target, *code).and_then(|()| {
            let toggled = toggled(source);
            let taken = seen.contains(source.as_str())
                || toggled.as_deref().is_some_and(|t| seen.contains(t));
            if !taken {
                seen.insert(source.as_str());
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
    // A source whose slash variant is stored is taken, as in `upsert` (R26).
    for (i, (source, _, _)) in rows.iter().enumerate() {
        if valid[i]
            && !map.contains_key(source)
            && toggled(source).is_some_and(|t| map.contains_key(&t))
        {
            valid[i] = false;
            errors.push(row_error(i, source_taken()));
        }
    }
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
    errors.sort_by_key(|e| e.row);
    errors
}

/// Upserts `rows` with origin `import`; no checks, no event. Counts created and updated.
pub(crate) async fn write_rows(
    repos: &dyn Repos,
    rows: &[(String, String, u16)],
) -> Result<ImportReport, CoreError> {
    let mut report = ImportReport {
        created: 0,
        updated: 0,
    };
    let now = Timestamp::now();
    for (source, target, code) in rows {
        let id = RedirectId::new();
        let saved = repos
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
    Ok(report)
}

/// [`RedirectService::import`] without the capability check, the transaction and the
/// event: all rows are validated against the stored redirects of `repos` (`InvalidRows`
/// lists the failing ones, nothing is written), then written. The caller emits the
/// `redirect:` keys of the sources.
pub(crate) async fn import_in(
    repos: &dyn Repos,
    rows: &[(String, String, u16)],
) -> Result<ImportReport, CoreError> {
    if rows.len() > MAX_IMPORT_ROWS {
        return Err(CoreError::Invalid(format!(
            "an import takes at most {MAX_IMPORT_ROWS} rows"
        )));
    }
    let errors = validate_rows(rows, all_targets_in(repos).await?);
    if !errors.is_empty() {
        return Err(CoreError::InvalidRows(errors));
    }
    write_rows(repos, rows).await
}

/// The read check of [`RedirectService::list`] and [`RedirectService::get`]: the role
/// holds `manage_redirects` (only admins see redirects), and `require_read(Redirects)`.
fn require_view(actor: &Actor) -> Result<(), CoreError> {
    if !can(actor.role, Capability::ManageRedirects) {
        return Err(CoreError::Forbidden(Capability::ManageRedirects.as_str()));
    }
    require_read(actor, Area::Redirects)
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

fn source_taken() -> CoreError {
    CoreError::invalid_field("source", "source taken by another redirect")
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
    let keys = redirect_keys(sources);
    if !keys.is_empty() {
        repos.events().append(&events::invalidate(keys)).await?;
    }
    Ok(())
}

/// `redirect:<source>` for each source and its toggled variant: the keys
/// [`emit_redirects_changed`] sends, for a caller that collects keys into its own event.
pub(crate) fn redirect_keys(sources: impl IntoIterator<Item = String>) -> Vec<String> {
    sources
        .into_iter()
        .flat_map(|s| {
            let variant = toggled(&s);
            std::iter::once(s).chain(variant)
        })
        .map(|s| events::redirect_key(&s))
        .collect()
}

/// Deletes the redirects that start at `path` or its slash-toggled variant: `path` is
/// a live page now, and [`RedirectService::resolve`]
/// matches either variant. Emits `redirect:` keys for the sources deleted.
pub(crate) async fn clear_redirects_from(repos: &dyn Repos, path: &str) -> Result<(), CoreError> {
    let cleared = delete_redirects_from(repos, path).await?;
    emit_redirects_changed(repos, cleared).await
}

/// Deletes the redirects from `path` and its toggled variant; returns their sources.
pub(crate) async fn delete_redirects_from(
    repos: &dyn Repos,
    path: &str,
) -> Result<Vec<String>, CoreError> {
    let redirects = repos.redirects();
    let mut cleared = Vec::new();
    for source in std::iter::once(path.to_owned()).chain(toggled(path)) {
        if let Some(r) = redirects.get_by_source(&source).await? {
            redirects.delete(r.id).await?;
            cleared.push(source);
        }
    }
    Ok(cleared)
}

/// Repoints redirects after a published item (or a term archive) moved from `old` to `new`: no redirect may
/// start at `new` or its slash variant (it is a live page again), redirects to `old` now
/// go to `new` (no chains; one that would start at `new` is dropped rather than left as
/// a self-loop), and `old` redirects to `new` with a `301`.
/// Emits one `invalidate` event with the `redirect:` keys of every source touched.
pub(crate) async fn record_move(repos: &dyn Repos, old: &str, new: &str) -> Result<(), CoreError> {
    let changed = move_redirects(repos, old, new).await?;
    emit_redirects_changed(repos, changed).await
}

/// [`record_move`] without the event: returns the sources of every redirect touched.
pub(crate) async fn move_redirects(
    repos: &dyn Repos,
    old: &str,
    new: &str,
) -> Result<Vec<String>, CoreError> {
    let mut changed = delete_redirects_from(repos, new).await?;
    changed.push(old.to_owned());
    let ours: Vec<String> = std::iter::once(new.to_owned())
        .chain(toggled(new))
        .collect();
    let redirects = repos.redirects();
    // Each pass re-targets (or drops) what it read, so the next pass sees the rest.
    loop {
        let batch = redirects.list_by_target(old).await?;
        let full = batch.len() >= MAX_REDIRECTS_BY_TARGET as usize;
        for mut r in batch {
            changed.push(r.source_path.clone());
            if ours.contains(&r.source_path) {
                redirects.delete(r.id).await?;
            } else {
                r.target = new.to_owned();
                redirects.upsert(&r).await?;
            }
        }
        if !full {
            break;
        }
    }
    redirects
        .upsert(&Redirect {
            id: RedirectId::new(),
            source_path: old.to_owned(),
            target: new.to_owned(),
            code: 301,
            hits: 0,
            origin: RedirectOrigin::SlugChange,
            created_at: Timestamp::now(),
            last_hit_at: None,
        })
        .await?;
    Ok(changed)
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
    if is_router_owned(source) {
        return Err(CoreError::invalid_field(
            "source",
            "redirect source is a path the site serves itself (API, assets, health, feeds, sitemap, robots.txt)",
        ));
    }
    Ok(())
}

/// Whether the first segment of `source` (percent-decoded, any case) is one the router
/// owns: a [`slug::RESERVED`] word (`/api`, `/feed/…`, `/_media`, `/healthz`,
/// `/robots.txt`, `/sitemap.xml`, …), any word starting with `_` (`/_theme`, `/_p`,
/// `/_preview`, …), or a sitemap shard name `sitemap-*.xml` (the router answers every
/// such name itself, 404 for an unknown type or number; R26). Redirects there either
/// never fire (dispatched first) or would shadow a built-in (redirect rows are checked
/// before the sitemap render). Only new writes are checked; stored rows keep working.
fn is_router_owned(source: &str) -> bool {
    let first = source
        .trim_start_matches('/')
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    let decoded = percent_encoding::percent_decode_str(first).decode_utf8_lossy();
    let word = decoded.to_lowercase();
    word.starts_with('_')
        || slug::is_reserved(&word)
        || (word.starts_with("sitemap-") && word.ends_with(".xml"))
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
    fn router_owned_sources() {
        for owned in [
            "/api",
            "/api/v1/x",
            "/API/",
            "/_media/a.png",
            "/_anything",
            "/%5Fpreview/x",
            "/healthz",
            "/readyz",
            "/robots.txt",
            "/sitemap.xml",
            "/feed/",
            "/feed",
            "/feed/atom/",
            "/sitemap-post-1.xml",
            "/Sitemap-Book-12.xml",
            "/sitemap-x.xml/",
            "/sitemap-%70ost-1.xml",
        ] {
            assert!(is_router_owned(owned), "{owned}");
            assert!(
                matches!(
                    check_source(owned),
                    Err(CoreError::InvalidField {
                        field: "source",
                        ..
                    })
                ),
                "{owned}"
            );
        }
        for free in [
            "/",
            "/apis",
            "/about/",
            "/feeds/",
            "/blog/_draft",
            "/my_page",
            "/sitemap-guide/",
            "/sitemaps.xml",
            "/sitemap-a.xml.bak",
        ] {
            assert!(!is_router_owned(free), "{free}");
        }
    }

    #[test]
    fn toggle_variants() {
        assert_eq!(toggled("/a").as_deref(), Some("/a/"));
        assert_eq!(toggled("/a/").as_deref(), Some("/a"));
        assert_eq!(toggled("/"), None);
        assert_eq!(toggled("a"), None);
    }
}
