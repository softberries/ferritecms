//! Redirect lookup and management.

use std::sync::Arc;

use ferrite_domain::{Page, Redirect, RedirectId, RedirectOrigin, Timestamp};
use ferrite_store::Store;

use crate::{
    Actor, CoreError,
    actor::{Capability, require},
    tx::finish,
};

/// Redirects followed when checking a new one for loops and long chains.
const MAX_HOPS: usize = 10;

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
        let repo = self.store.read().redirects();
        if let Some(r) = repo.get_by_source(path).await? {
            return Ok(Some(r));
        }
        let Some(other) = toggled(path) else {
            return Ok(None);
        };
        Ok(repo.get_by_source(&other).await?)
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
    ///
    /// Rejected as `Invalid`: `source == target`; a target that leads back to `source`
    /// through existing redirects (loop); a target that starts a chain of more than 10
    /// existing redirects. Chains are followed by exact source match.
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
        if source == target {
            return Err(CoreError::Invalid("redirect to itself".into()));
        }
        let tx = self.store.begin().await?;
        let done = async {
            let mut at = target.to_owned();
            for _ in 0..MAX_HOPS {
                let Some(next) = tx.redirects().get_by_source(&at).await? else {
                    return Ok(());
                };
                if next.target == source {
                    return Err(CoreError::Invalid("redirect loop".into()));
                }
                at = next.target;
            }
            if tx.redirects().get_by_source(&at).await?.is_some() {
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

/// `/a` <-> `/a/`; `None` for `/` and for paths without a leading `/`.
fn toggled(path: &str) -> Option<String> {
    if path == "/" || !path.starts_with('/') {
        return None;
    }
    Some(match path.strip_suffix('/') {
        Some(stripped) => stripped.to_owned(),
        None => format!("{path}/"),
    })
}

fn check_source(source: &str) -> Result<(), CoreError> {
    if !source.starts_with('/') || source.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(CoreError::Invalid(
            "redirect source must be a path starting with '/'".into(),
        ));
    }
    Ok(())
}

fn check_target(target: &str) -> Result<(), CoreError> {
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
