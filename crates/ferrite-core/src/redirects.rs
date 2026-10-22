//! Redirect lookup and management.

use std::sync::Arc;

use ferrite_domain::{Page, Redirect, RedirectId, RedirectOrigin, Timestamp};
use ferrite_store::{Repos, Store};

use crate::{
    Actor, CoreError,
    actor::{Capability, require},
    tx::finish,
};

/// Redirects followed when checking a new one for loops and long chains.
const MAX_HOPS: usize = 10;
/// Longest source or target accepted.
const MAX_URL_LEN: usize = 2048;

/// Resolves and manages redirects (writes need `manage_redirects`).
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

    /// Creates a manual redirect from `source` (a path starting with `/`) to `target`
    /// (a path or an absolute http(s) URL), or changes the existing one for `source`.
    /// Both are at most 2048 characters.
    ///
    /// Rejected as `Invalid`: `source == target`; a target that leads back to `source`
    /// through existing redirects (loop); a target that starts a chain of more than 10
    /// existing redirects. Hops are followed the way [`resolve`](Self::resolve) matches
    /// them (exact path, then trailing slash toggled), and `source` counts together with
    /// its toggled variant. The chain limit counts downstream hops only; redirects
    /// that already lead into `source` are not counted.
    ///
    /// Absolute targets let a redirect send visitors to any site (an open redirect), which
    /// is why this is admin-only (`manage_redirects`). Under the default isolation
    /// (READ COMMITTED on Postgres) the check is not serialized: two concurrent upserts
    /// can still close a loop together (plan ruling P2).
    pub async fn upsert(
        &self,
        actor: &Actor,
        source: &str,
        target: &str,
        code: u16,
    ) -> Result<Redirect, CoreError> {
        require(actor, Capability::ManageRedirects)?;
        check_source(source)?;
        check_target(target)?;
        let mut ours = vec![source.to_owned()];
        ours.extend(toggled(source));
        if ours.iter().any(|v| v == target) {
            return Err(CoreError::Invalid("redirect to itself".into()));
        }
        let tx = self.store.begin().await?;
        let done = async {
            let mut at = target.to_owned();
            for _ in 0..MAX_HOPS {
                let Some(next) = lookup(tx.as_ref(), &at).await? else {
                    return Ok(());
                };
                if ours.contains(&next.target) {
                    return Err(CoreError::Invalid("redirect loop".into()));
                }
                at = next.target;
            }
            if lookup(tx.as_ref(), &at).await?.is_some() {
                return Err(CoreError::Invalid(format!(
                    "redirect chain longer than {MAX_HOPS}"
                )));
            }
            Ok(())
        }
        .await;
        let done = match done {
            Ok(()) => tx
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
                .await
                .map_err(CoreError::from),
            Err(e) => Err(e),
        };
        finish(tx, done).await
    }

    /// Deletes a redirect (a missing one is a no-op).
    pub async fn delete(&self, actor: &Actor, id: RedirectId) -> Result<(), CoreError> {
        require(actor, Capability::ManageRedirects)?;
        let tx = self.store.begin().await?;
        let done = tx.redirects().delete(id).await.map_err(CoreError::from);
        finish(tx, done).await
    }
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
        return Err(CoreError::Invalid(format!(
            "redirect source longer than {MAX_URL_LEN} characters"
        )));
    }
    if !source.starts_with('/') || source.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(CoreError::Invalid(
            "redirect source must be a path starting with '/'".into(),
        ));
    }
    Ok(())
}

fn check_target(target: &str) -> Result<(), CoreError> {
    if target.chars().count() > MAX_URL_LEN {
        return Err(CoreError::Invalid(format!(
            "redirect target longer than {MAX_URL_LEN} characters"
        )));
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
        return Err(CoreError::Invalid(
            "redirect target must be a path or an http(s) URL".into(),
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
