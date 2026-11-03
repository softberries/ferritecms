//! Approval workflows (design 15 §6): per content type, ordered review steps with
//! required approvals before anything of that type is published.

pub mod gate;
mod review;

use std::{collections::HashSet, sync::Arc};

use ferrite_domain::{
    MAX_PAGE, MEDIA_KIND, Role, Timestamp, UserId, UserQuery, UserStatus, Workflow, WorkflowStep,
};
use ferrite_store::{Store, StoreError};

use crate::{
    Actor, CoreError,
    actor::{Area, Capability, require, require_read},
    tx::finish,
};

pub use gate::{MAX_OVERRIDE_REASON, ReviewSummary, WorkflowState, state_of};
pub use review::{MAX_ASSIGNEES, MAX_REVIEW_TEXT, ReviewDetail, ReviewService, SubmitInput};
pub(crate) use review::{mark_stale_item, start_in};

/// Most steps per workflow.
pub const MAX_STEPS: usize = 5;
/// Most named users per step.
pub const MAX_STEP_USERS: usize = 20;
/// Most approvals a step may require.
pub const MAX_REQUIRED: u32 = 5;
/// Longest step label.
pub const MAX_STEP_LABEL: usize = 80;
/// Longest step key.
pub const MAX_STEP_KEY: usize = 40;
/// Most approvers [`WorkflowService::approvers`] lists.
pub const MAX_APPROVERS_LISTED: usize = 200;

/// Someone who may approve a step: what a reviewer picker shows, nothing more.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Approver {
    pub id: UserId,
    /// The display name, or the login when it is empty.
    pub display_name: String,
}

/// The preset "One editor approves" (one step, editors and admins, one approval).
pub fn one_editor_preset() -> Vec<WorkflowStep> {
    vec![WorkflowStep {
        key: "review".into(),
        label: "Editor review".into(),
        approvers: ferrite_domain::Approvers {
            roles: vec![Role::Editor, Role::Admin],
            users: vec![],
        },
        required: 1,
    }]
}

/// Reads and writes workflows (admins: `manage_settings`).
pub struct WorkflowService {
    store: Arc<dyn Store>,
}

impl WorkflowService {
    pub fn new(store: Arc<dyn Store>) -> Self {
        Self { store }
    }

    /// The workflow of a type (any reader of content may see it: the editor needs it).
    pub async fn get(&self, actor: &Actor, type_name: &str) -> Result<Workflow, CoreError> {
        require_read(actor, Area::Content)?;
        self.store
            .read()
            .workflows()
            .get(type_name)
            .await?
            .ok_or(CoreError::NotFound)
    }

    /// The active users who may approve step `step` of `type_name`'s workflow (named by
    /// the step's roles or users, with `edit_others`), by display name, at most
    /// [`MAX_APPROVERS_LISTED`]: the reviewer pickers. Anyone who may submit for review
    /// (`edit_own`, a content read scope) may list them; `NotFound` without a workflow,
    /// `InvalidField("step")` for a step it does not have.
    pub async fn approvers(
        &self,
        actor: &Actor,
        type_name: &str,
        step: u32,
    ) -> Result<Vec<Approver>, CoreError> {
        require(actor, Capability::EditOwn)?;
        require_read(actor, Area::Content)?;
        let repos = self.store.read();
        let w = repos
            .workflows()
            .get(type_name)
            .await?
            .ok_or(CoreError::NotFound)?;
        let s = w
            .steps
            .get(step as usize)
            .ok_or_else(|| CoreError::invalid_field("step", "no such step"))?;
        let mut users = repos.users().get_many(&s.approvers.users).await?;
        for role in &s.approvers.roles {
            let mut cursor = None;
            loop {
                let page = repos
                    .users()
                    .list(&UserQuery {
                        q: None,
                        role: Some(*role),
                        status: Some(UserStatus::Active),
                        limit: MAX_PAGE,
                        cursor,
                    })
                    .await?;
                users.extend(page.items);
                cursor = page.next_cursor;
                if cursor.is_none() || users.len() > MAX_APPROVERS_LISTED * 2 {
                    break;
                }
            }
        }
        let mut seen = HashSet::new();
        let mut out: Vec<Approver> = users
            .into_iter()
            .filter(|u| u.status == UserStatus::Active && review::eligible(s, u.id, u.role))
            .filter(|u| seen.insert(u.id))
            .map(|u| Approver {
                id: u.id,
                display_name: if u.display_name.trim().is_empty() {
                    u.login
                } else {
                    u.display_name
                },
            })
            .collect();
        out.sort_by(|a, b| a.display_name.cmp(&b.display_name).then(a.id.cmp(&b.id)));
        out.truncate(MAX_APPROVERS_LISTED);
        Ok(out)
    }

    /// Every workflow, by type name.
    pub async fn list(&self, actor: &Actor) -> Result<Vec<Workflow>, CoreError> {
        require_read(actor, Area::Content)?;
        Ok(self.store.read().workflows().list().await?)
    }

    /// Creates (`version` `None`) or replaces (`Some(v)`, `If-Match`) the workflow of
    /// `type_name`. Steps are validated as §6.1 says; open reviews keep the steps they
    /// started with (D23).
    pub async fn put(
        &self,
        actor: &Actor,
        type_name: &str,
        steps: Vec<WorkflowStep>,
        version: Option<i64>,
    ) -> Result<Workflow, CoreError> {
        require(actor, Capability::ManageSettings)?;
        let tx = self.store.begin().await?;
        let done = async {
            let ty = tx
                .types()
                .get(type_name)
                .await?
                .ok_or(CoreError::NotFound)?;
            if ty.name == MEDIA_KIND {
                return Err(CoreError::Invalid("media cannot have a workflow".into()));
            }
            let steps = validate_steps(tx.as_ref(), steps).await?;
            let w = Workflow {
                type_name: ty.name,
                steps,
                updated_by: Some(actor.user),
                updated_at: Timestamp::now(),
                version: version.unwrap_or(0),
            };
            match tx.workflows().put(&w, version).await {
                Err(StoreError::UniqueViolation { .. }) => Err(CoreError::Invalid(
                    "the type has a workflow: send If-Match".into(),
                )),
                other => Ok(other?),
            }
        }
        .await;
        finish(tx, done).await
    }

    /// Removes the workflow of `type_name` at `version`; its open reviews are withdrawn.
    pub async fn delete(
        &self,
        actor: &Actor,
        type_name: &str,
        version: i64,
    ) -> Result<(), CoreError> {
        require(actor, Capability::ManageSettings)?;
        let tx = self.store.begin().await?;
        let done = async {
            let w = tx
                .workflows()
                .get(type_name)
                .await?
                .ok_or(CoreError::NotFound)?;
            if w.version != version {
                return Err(CoreError::Conflict {
                    current_version: w.version,
                });
            }
            tx.reviews()
                .withdraw_for_type(type_name, Timestamp::now())
                .await?;
            tx.workflows().delete(type_name).await?;
            Ok(())
        }
        .await;
        finish(tx, done).await
    }
}

/// The checks of §6.1, returning the steps with trimmed labels and sorted roles.
pub(crate) async fn validate_steps(
    repos: &dyn ferrite_store::Repos,
    steps: Vec<WorkflowStep>,
) -> Result<Vec<WorkflowStep>, CoreError> {
    let bad = |m: String| CoreError::invalid_field("steps", m);
    if steps.is_empty() || steps.len() > MAX_STEPS {
        return Err(bad(format!("1 to {MAX_STEPS} steps")));
    }
    let mut keys = HashSet::new();
    let mut out = Vec::with_capacity(steps.len());
    for (i, mut s) in steps.into_iter().enumerate() {
        let n = i + 1;
        let key_ok = !s.key.is_empty()
            && s.key.len() <= MAX_STEP_KEY
            && s.key
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if !key_ok {
            return Err(bad(format!(
                "step {n}: the key is 1 to {MAX_STEP_KEY} of a-z, 0-9 and -"
            )));
        }
        if !keys.insert(s.key.clone()) {
            return Err(bad(format!("step {n}: the key {} is used twice", s.key)));
        }
        s.label = s.label.trim().to_owned();
        if s.label.is_empty()
            || s.label.chars().count() > MAX_STEP_LABEL
            || s.label.chars().any(char::is_control)
        {
            return Err(bad(format!(
                "step {n}: the label is 1 to {MAX_STEP_LABEL} characters"
            )));
        }
        if s.required == 0 || s.required > MAX_REQUIRED {
            return Err(bad(format!("step {n}: 1 to {MAX_REQUIRED} approvals")));
        }
        if s.approvers
            .roles
            .iter()
            .any(|r| !matches!(r, Role::Editor | Role::Admin))
        {
            return Err(bad(format!(
                "step {n}: approver roles are editor and admin"
            )));
        }
        s.approvers.roles.sort_by_key(|r| r.as_str());
        s.approvers.roles.dedup();
        let mut seen = HashSet::new();
        s.approvers.users.retain(|u| seen.insert(*u));
        if s.approvers.users.len() > MAX_STEP_USERS {
            return Err(bad(format!("step {n}: at most {MAX_STEP_USERS} users")));
        }
        if s.approvers.roles.is_empty() && s.approvers.users.is_empty() {
            return Err(bad(format!("step {n}: name approver roles or users")));
        }
        let users = repos.users().get_many(&s.approvers.users).await?;
        for id in &s.approvers.users {
            let ok = users.iter().any(|u| {
                u.id == *id
                    && u.status == UserStatus::Active
                    && matches!(u.role, Role::Editor | Role::Admin)
            });
            if !ok {
                return Err(bad(format!(
                    "step {n}: approvers are active editors or administrators"
                )));
            }
        }
        out.push(s);
    }
    Ok(out)
}
