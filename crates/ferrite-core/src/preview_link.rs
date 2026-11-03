//! Signed, expiring preview links (design 15 §7, D26, D27): `ferp_` + 43 base64url
//! characters (32 random bytes), stored as the sha-256 hex of the token, rendering as
//! their creator (re-checked on every use) with `content:read` only.

use std::{sync::Arc, time::Duration};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ferrite_domain::{
    ContentItem, NewAudit, PreviewLink, PreviewLinkId, PreviewTarget, Release, Timestamp,
    UserStatus,
};
use ferrite_store::{Repos, Store};
use serde_json::json;

use crate::{
    Actor, CoreError, Scope, Scopes,
    actor::{Area, Capability, can, can_read, require, require_read},
    auth::limits,
    auth::{random_bytes, sha256_hex},
};

/// The token prefix.
pub const PREVIEW_TOKEN_PREFIX: &str = "ferp_";
/// Characters after the prefix (32 bytes in unpadded base64url).
pub const PREVIEW_TOKEN_CHARS: usize = 43;
/// Longest life of a link (D26).
pub const PREVIEW_LINK_MAX_HOURS: u32 = 168;
/// Life of a link when none is asked for.
pub const PREVIEW_LINK_DEFAULT_HOURS: u32 = 24;
/// Links one user creates per hour.
pub const PREVIEW_LINKS_PER_HOUR: u32 = 30;
/// Renders one link serves per minute (D34).
pub const PREVIEW_RENDERS_PER_MINUTE: u32 = 60;
/// `last_used_at` is written at most this often.
const TOUCH_EVERY_MICROS: i64 = 60_000_000;

/// Whether `s` has the shape of a preview token (the route grammar's check).
pub fn is_token(s: &str) -> bool {
    s.strip_prefix(PREVIEW_TOKEN_PREFIX).is_some_and(|rest| {
        rest.len() == PREVIEW_TOKEN_CHARS
            && rest
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    })
}

/// A resolved link: the link, the creator's actor (`content:read` only) and the target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewPrincipal {
    pub link: PreviewLink,
    pub actor: Actor,
}

/// Creates, lists, revokes and resolves preview links.
pub struct PreviewLinkService {
    store: Arc<dyn Store>,
}

/// Whether `actor` may preview `target`: `can_read` for an item, release visibility
/// for a release.
async fn may_preview(
    repos: &dyn Repos,
    actor: &Actor,
    target: &PreviewTarget,
) -> Result<bool, CoreError> {
    Ok(match target {
        PreviewTarget::Item(id) => repos
            .content()
            .get(*id)
            .await?
            .is_some_and(|i: ContentItem| {
                i.status != ferrite_domain::ContentStatus::Trash
                    && can_read(actor, &i, Timestamp::now())
            }),
        PreviewTarget::Release(id) => repos.releases().get(*id).await?.is_some_and(|r: Release| {
            r.status != ferrite_domain::ReleaseStatus::Applied && crate::release::may_see(actor, &r)
        }),
    })
}

impl PreviewLinkService {
    pub fn new(store: Arc<dyn Store>) -> Self {
        Self { store }
    }

    /// Creates a link to `target` living `hours` (1–168, default 24) and returns it with
    /// its token (shown once). The creator must be able to preview the target; 30 links
    /// per user and hour; audited as `preview_link.create`.
    pub async fn create(
        &self,
        actor: &Actor,
        target: PreviewTarget,
        hours: Option<u32>,
    ) -> Result<(PreviewLink, String), CoreError> {
        require(actor, Capability::EditOwn)?;
        let hours = hours.unwrap_or(PREVIEW_LINK_DEFAULT_HOURS);
        if hours == 0 || hours > PREVIEW_LINK_MAX_HOURS {
            return Err(CoreError::invalid_field(
                "expires_in_hours",
                format!("1 to {PREVIEW_LINK_MAX_HOURS} hours"),
            ));
        }
        if !may_preview(self.store.read(), actor, &target).await? {
            return Err(CoreError::NotFound);
        }
        limits::hit(
            self.store.as_ref(),
            &format!("preview_link:user:{}", actor.user),
            PREVIEW_LINKS_PER_HOUR,
            Duration::from_secs(3600),
        )
        .await?;
        let token = format!(
            "{PREVIEW_TOKEN_PREFIX}{}",
            URL_SAFE_NO_PAD.encode(random_bytes::<32>()?)
        );
        let now = Timestamp::now();
        let link = PreviewLink {
            id: PreviewLinkId::new(),
            target,
            created_by: actor.user,
            created_at: now,
            expires_at: Timestamp::from_micros(now.as_micros() + i64::from(hours) * 3_600_000_000)
                .map_err(|_| CoreError::Internal("preview link expiry out of range"))?,
            last_used_at: None,
        };
        let tx = self.store.begin().await?;
        let done = async {
            tx.preview_links()
                .insert(&link, &sha256_hex(token.as_bytes()))
                .await?;
            tx.audit()
                .append(&NewAudit {
                    actor_id: Some(actor.user),
                    action: "preview_link.create".into(),
                    target_kind: target.kind().as_str().into(),
                    target_id: Some(target.id().to_string()),
                    details: json!({"link": link.id.to_string(), "hours": hours}),
                })
                .await?;
            Ok(())
        }
        .await;
        crate::tx::finish(tx, done).await?;
        Ok((link, token))
    }

    /// The links of `target` (the actor must be able to preview it).
    pub async fn list(
        &self,
        actor: &Actor,
        target: PreviewTarget,
    ) -> Result<Vec<PreviewLink>, CoreError> {
        require_read(actor, Area::Content)?;
        let repos = self.store.read();
        if !may_preview(repos, actor, &target).await? {
            return Err(CoreError::NotFound);
        }
        let mut links = repos.preview_links().list_for(&target).await?;
        if !can(actor.role, Capability::EditOthers) {
            links.retain(|l| l.created_by == actor.user);
        }
        Ok(links)
    }

    /// Revokes link `id` (its creator or `edit_others`).
    pub async fn revoke(&self, actor: &Actor, id: PreviewLinkId) -> Result<(), CoreError> {
        require(actor, Capability::EditOwn)?;
        let repos = self.store.read();
        let link = repos
            .preview_links()
            .get(id)
            .await?
            .ok_or(CoreError::NotFound)?;
        if link.created_by != actor.user && !can(actor.role, Capability::EditOthers) {
            return Err(CoreError::NotFound);
        }
        repos.preview_links().delete(id).await?;
        Ok(())
    }

    /// The principal of `token`, re-checked now (D27): known and unexpired, its creator
    /// active and not `must_reset`, still allowed to preview the target. `None`
    /// otherwise. Touches `last_used_at` at most once a minute.
    pub async fn resolve(&self, token: &str) -> Result<Option<PreviewPrincipal>, CoreError> {
        if !is_token(token) {
            return Ok(None);
        }
        let repos = self.store.read();
        let Some(link) = repos
            .preview_links()
            .get_by_hash(&sha256_hex(token.as_bytes()))
            .await?
        else {
            return Ok(None);
        };
        let now = Timestamp::now();
        if link.expires_at <= now {
            return Ok(None);
        }
        let Some(user) = repos.users().get(link.created_by).await? else {
            return Ok(None);
        };
        if user.status != UserStatus::Active || user.must_reset {
            return Ok(None);
        }
        let actor =
            Actor::new(user.id, user.role).with_scopes(Scopes::only(Scope::read(Area::Content)));
        if !may_preview(repos, &actor, &link.target).await? {
            return Ok(None);
        }
        if link
            .last_used_at
            .is_none_or(|t| now.as_micros() - t.as_micros() >= TOUCH_EVERY_MICROS)
        {
            // Best effort: a busy writer must not turn a valid link away.
            if let Err(e) = repos.preview_links().touch(link.id, now).await {
                tracing::debug!(error = %e, "preview link touch skipped");
            }
        }
        Ok(Some(PreviewPrincipal { link, actor }))
    }

    /// Counts one render of `link` (60 a minute, D34): `RateLimited` beyond.
    pub async fn hit_render(&self, link: PreviewLinkId) -> Result<(), CoreError> {
        limits::hit(
            self.store.as_ref(),
            &format!("preview:{link}"),
            PREVIEW_RENDERS_PER_MINUTE,
            Duration::from_secs(60),
        )
        .await
    }
}
