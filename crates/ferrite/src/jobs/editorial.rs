//! M6-3 editorial jobs (design 15 §9.3): `release.apply`, the `release.apply_due`
//! periodic, `editorial.notify` mail and the daily `preview_links.prune`.

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use ferrite_cluster::{JobError, JobHandler, Periodic};
use ferrite_core::{
    CoreError, is_bidi_control,
    jobs::{
        EDITORIAL_NOTIFY_KIND, EDITORIAL_NOTIFY_VERSION, EditorialNotify, PREVIEW_LINKS_PRUNE_KIND,
        RELEASE_APPLY_DUE_KIND, RELEASE_APPLY_KIND, RELEASE_APPLY_VERSION, ReleaseApply,
    },
    mail::{Mailer, OutgoingMail, check_address, subject_title},
    release::ReleaseService,
};
use ferrite_domain::{Job, Role, Timestamp, User, UserId, UserQuery, UserStatus};
use ferrite_store::{MAX_PRUNE_BATCH, Store};
use serde_json::json;
use url::Url;

/// Period of the [`RELEASE_APPLY_DUE_KIND`] safety net.
pub const RELEASE_APPLY_DUE_EVERY: Duration = Duration::from_secs(30);
/// Period of [`PREVIEW_LINKS_PRUNE_KIND`].
pub const PREVIEW_LINKS_PRUNE_EVERY: Duration = Duration::from_secs(24 * 3600);
/// Most recipients of one editorial mail (bcc).
pub const MAX_RECIPIENTS: usize = 50;

fn core_to_job(e: CoreError) -> JobError {
    match e {
        CoreError::Store(_) | CoreError::Unavailable(_) | CoreError::Random => {
            JobError::Retry(e.to_string())
        }
        other => JobError::Permanent(other.to_string()),
    }
}

/// `release.apply`: applies the payload's release if it is still scheduled for the
/// payload's time ([`ReleaseService::apply_scheduled`]); a permanent failure marks the
/// release failed inside core, so only store errors reach the runner (retried).
pub struct ReleaseApplyHandler {
    pub releases: Arc<ReleaseService>,
}

#[async_trait]
impl JobHandler for ReleaseApplyHandler {
    fn kind(&self) -> &'static str {
        RELEASE_APPLY_KIND
    }

    fn versions(&self) -> &'static [u32] {
        &[RELEASE_APPLY_VERSION]
    }

    async fn run(&self, job: &Job) -> Result<(), JobError> {
        let payload: ReleaseApply = serde_json::from_value(job.payload.clone())
            .map_err(|e| JobError::Permanent(format!("invalid payload: {e}")))?;
        let applied = self
            .releases
            .apply_scheduled(payload.id, payload.at)
            .await
            .map_err(core_to_job)?;
        if applied {
            tracing::info!(release = %payload.id, "applied scheduled release");
        }
        Ok(())
    }
}

/// `release.apply_due`: applies every release due by the database clock.
pub struct ReleaseApplyDueHandler {
    pub store: Arc<dyn Store>,
    pub releases: Arc<ReleaseService>,
}

#[async_trait]
impl JobHandler for ReleaseApplyDueHandler {
    fn kind(&self) -> &'static str {
        RELEASE_APPLY_DUE_KIND
    }

    async fn run(&self, _job: &Job) -> Result<(), JobError> {
        let now = self
            .store
            .read()
            .nodes()
            .db_now()
            .await
            .map_err(|e| JobError::Retry(e.to_string()))?;
        let n = self
            .releases
            .apply_due(now)
            .await
            .map_err(|e| JobError::Retry(e.to_string()))?;
        if n > 0 {
            tracing::info!(applied = n, "applied due releases");
        }
        Ok(())
    }
}

/// `preview_links.prune`: deletes expired preview links.
pub struct PreviewLinksPruneHandler {
    pub store: Arc<dyn Store>,
}

#[async_trait]
impl JobHandler for PreviewLinksPruneHandler {
    fn kind(&self) -> &'static str {
        PREVIEW_LINKS_PRUNE_KIND
    }

    async fn run(&self, _job: &Job) -> Result<(), JobError> {
        let mut total = 0;
        loop {
            let n = self
                .store
                .read()
                .preview_links()
                .prune(Timestamp::now(), MAX_PRUNE_BATCH)
                .await
                .map_err(|e| JobError::Retry(e.to_string()))?;
            total += n;
            if n < u64::from(MAX_PRUNE_BATCH) {
                break;
            }
        }
        if total > 0 {
            tracing::info!(deleted = total, "pruned expired preview links");
        }
        Ok(())
    }
}

/// `editorial.notify`: computes the recipients and text when it runs (D24) and sends
/// through `mailer`; without `[mail]` (`mailer` `None`) it completes with a debug log.
pub struct EditorialNotifyHandler {
    pub store: Arc<dyn Store>,
    pub mailer: Option<Arc<dyn Mailer>>,
    pub site_url: Url,
    /// `site.title` from the config, when the option is unset.
    pub site_title: String,
}

/// A title for a subject: control and bidi characters removed, at most 100 characters.
fn clean(title: &str) -> String {
    title
        .chars()
        .filter(|c| !c.is_control() && !is_bidi_control(*c))
        .take(100)
        .collect()
}

fn addresses(users: impl IntoIterator<Item = User>, skip: &[UserId]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for u in users {
        if u.status != UserStatus::Active
            || skip.contains(&u.id)
            || check_address(&u.email).is_err()
            || out.contains(&u.email)
        {
            continue;
        }
        out.push(u.email);
        if out.len() >= MAX_RECIPIENTS {
            break;
        }
    }
    out
}

impl EditorialNotifyHandler {
    async fn users(&self, ids: &[UserId]) -> Result<Vec<User>, JobError> {
        self.store
            .read()
            .users()
            .get_many(ids)
            .await
            .map_err(|e| JobError::Retry(e.to_string()))
    }

    async fn by_role(&self, role: Role) -> Result<Vec<User>, JobError> {
        Ok(self
            .store
            .read()
            .users()
            .list(&UserQuery {
                role: Some(role),
                status: Some(UserStatus::Active),
                limit: MAX_RECIPIENTS as u32,
                ..Default::default()
            })
            .await
            .map_err(|e| JobError::Retry(e.to_string()))?
            .items)
    }

    /// The mail of `payload`, or `None` when it has no recipient or its row is gone.
    pub async fn build(&self, payload: &EditorialNotify) -> Result<Option<OutgoingMail>, JobError> {
        let site = self.site_url.as_str().trim_end_matches('/');
        let prefix = subject_title(&self.site_title).map_or(String::new(), |t| format!("[{t}] "));
        let repos = self.store.read();
        if let Some(rid) = payload.review {
            let Some(review) = repos
                .reviews()
                .get(rid)
                .await
                .map_err(|e| JobError::Retry(e.to_string()))?
            else {
                return Ok(None);
            };
            let Some(item) = repos
                .content()
                .get(review.target.item())
                .await
                .map_err(|e| JobError::Retry(e.to_string()))?
            else {
                return Ok(None);
            };
            let title = clean(&item.title);
            let link = format!("{site}/admin/reviews?review={rid}");
            let (to, bcc, subject, text) = match payload.event.as_str() {
                "review.submitted" | "review.step" => {
                    let step = review.step;
                    let assigned: Vec<UserId> = repos
                        .reviews()
                        .assignees(rid)
                        .await
                        .map_err(|e| JobError::Retry(e.to_string()))?
                        .into_iter()
                        .filter(|(s, _)| *s == step)
                        .map(|(_, u)| u)
                        .collect();
                    let mut people = self.users(&assigned).await?;
                    if people.is_empty()
                        && let Some(s) = review.steps.get(step as usize)
                    {
                        for role in &s.approvers.roles {
                            people.extend(self.by_role(*role).await?);
                        }
                        people.extend(self.users(&s.approvers.users).await?);
                    }
                    let mut skip = vec![item.author_id];
                    skip.extend(review.submitted_by);
                    let label = review
                        .steps
                        .get(step as usize)
                        .map(|s| clean(&s.label))
                        .unwrap_or_default();
                    (
                        Vec::new(),
                        addresses(people, &skip),
                        format!("{prefix}Review requested: {title}"),
                        format!(
                            "\"{title}\" waits for your review ({label}).\n\nOpen the review:\n{link}\n"
                        ),
                    )
                }
                "review.approved" | "review.changes_requested" => {
                    let who = self
                        .users(&review.submitted_by.into_iter().collect::<Vec<_>>())
                        .await?;
                    let approved = payload.event == "review.approved";
                    let comment = if approved {
                        String::new()
                    } else {
                        repos
                            .reviews()
                            .decisions(rid)
                            .await
                            .map_err(|e| JobError::Retry(e.to_string()))?
                            .into_iter()
                            .rev()
                            .find_map(|d| d.comment)
                            .map(|c| format!("\nComment:\n{c}\n"))
                            .unwrap_or_default()
                    };
                    (
                        addresses(who, &[]),
                        Vec::new(),
                        if approved {
                            format!("{prefix}Approved: {title}")
                        } else {
                            format!("{prefix}Changes requested: {title}")
                        },
                        format!(
                            "\"{title}\" was {}.\n{comment}\nOpen the review:\n{link}\n",
                            if approved {
                                "approved"
                            } else {
                                "sent back with requested changes"
                            }
                        ),
                    )
                }
                _ => return Ok(None),
            };
            return Ok(mail(to, bcc, subject, text));
        }
        if let Some(id) = payload.release {
            let Some(r) = repos
                .releases()
                .get(id)
                .await
                .map_err(|e| JobError::Retry(e.to_string()))?
            else {
                return Ok(None);
            };
            let title = clean(&r.title);
            let link = format!("{site}/admin/releases/{id}");
            let (ids, subject, text) = match payload.event.as_str() {
                "release.applied" => (
                    r.created_by.into_iter().collect::<Vec<_>>(),
                    format!("{prefix}Release applied: {title}"),
                    format!("The release \"{title}\" was applied.\n\n{link}\n"),
                ),
                "release.failed" => {
                    let reason = r
                        .failure
                        .as_ref()
                        .and_then(|f| f["reason"].as_str())
                        .map(clean)
                        .unwrap_or_default();
                    (
                        r.created_by.into_iter().chain(r.scheduled_by).collect(),
                        format!("{prefix}Release failed: {title}"),
                        format!(
                            "The scheduled release \"{title}\" could not be applied: {reason}\n\
                             Nothing of it was published; its items are unlocked.\n\n\
                             See the report:\n{link}\n"
                        ),
                    )
                }
                _ => return Ok(None),
            };
            return Ok(mail(
                addresses(self.users(&ids).await?, &[]),
                Vec::new(),
                subject,
                text,
            ));
        }
        Ok(None)
    }
}

fn mail(to: Vec<String>, bcc: Vec<String>, subject: String, text: String) -> Option<OutgoingMail> {
    if to.is_empty() && bcc.is_empty() {
        return None;
    }
    Some(OutgoingMail {
        to,
        bcc,
        subject,
        text,
    })
}

#[async_trait]
impl JobHandler for EditorialNotifyHandler {
    fn kind(&self) -> &'static str {
        EDITORIAL_NOTIFY_KIND
    }

    fn versions(&self) -> &'static [u32] {
        &[EDITORIAL_NOTIFY_VERSION]
    }

    async fn run(&self, job: &Job) -> Result<(), JobError> {
        let payload: EditorialNotify = serde_json::from_value(job.payload.clone())
            .map_err(|e| JobError::Permanent(format!("invalid payload: {e}")))?;
        let Some(mailer) = &self.mailer else {
            tracing::debug!(event = payload.event, "editorial mail skipped: no [mail]");
            return Ok(());
        };
        let Some(mail) = self.build(&payload).await? else {
            return Ok(());
        };
        mailer
            .send(mail)
            .await
            .map_err(|e| JobError::Retry(e.to_string()))
    }
}

/// The [`RELEASE_APPLY_DUE_KIND`] periodic, every [`RELEASE_APPLY_DUE_EVERY`].
pub fn release_apply_due_periodic() -> Periodic {
    Periodic {
        kind: RELEASE_APPLY_DUE_KIND,
        every: RELEASE_APPLY_DUE_EVERY,
        payload: json!({"v": 1}),
        max_attempts: ferrite_domain::DEFAULT_MAX_ATTEMPTS,
        offset: Duration::ZERO,
    }
}

/// The [`PREVIEW_LINKS_PRUNE_KIND`] periodic, daily.
pub fn preview_links_prune_periodic() -> Periodic {
    Periodic {
        kind: PREVIEW_LINKS_PRUNE_KIND,
        every: PREVIEW_LINKS_PRUNE_EVERY,
        payload: json!({"v": 1}),
        max_attempts: ferrite_domain::DEFAULT_MAX_ATTEMPTS,
        offset: Duration::from_secs(4 * 3600),
    }
}
