//! What an actor may do with one item right now ([`allowed_actions`], M2b P13): the
//! admin shows exactly the actions the server would accept, and never decides on its
//! own.
//!
//! Each flag is the conjunction of the permission check the matching method makes and
//! the transition being legal from the item's status, computed with the same
//! predicates the lifecycle and `update` call ([`authorize`], [`authorize_ownership`],
//! [`can_edit`], [`legal`], [`editable`], [`deletable`]), so the two cannot drift. In a
//! type with an approval workflow (M6-3) the gate's state comes in as
//! [`WorkflowState`].

use ferrite_domain::{ContentItem, ContentStatus, Role, Timestamp};

use super::lifecycle::{Action, authorize, authorize_ownership, deletable, editable, legal};
use crate::{Actor, actor::can_edit, workflow::WorkflowState};

/// The actions `actor` may take on an item ([`allowed_actions`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AllowedActions {
    /// `update` (and revision restore): edit rights and not trashed; in a workflow type
    /// a public item only for approvers (P7).
    pub edit: bool,
    /// `publish` (also "publish now" of a scheduled item).
    pub publish: bool,
    pub unpublish: bool,
    /// `schedule` (for a future time).
    pub schedule: bool,
    /// `submit` for review (also "Resubmit" of a pending item of a workflow type
    /// without an open review).
    pub submit: bool,
    pub trash: bool,
    /// `restore` from trash.
    pub restore: bool,
    /// Permanent `delete` of a trashed item. An item that still has children is
    /// refused by `delete` (`Invalid("has children")`) even when this is `true`.
    pub delete: bool,
    /// `trash`, and `restore` would be allowed once trashed: the admin offers Undo after
    /// a trash only then, and asks first otherwise (07 §3; contributors may trash their
    /// own open items but never restore them).
    pub trash_undoable: bool,
    /// The actor could publish or schedule but the workflow's approval is missing.
    pub needs_approval: bool,
    /// The item can be changed through a release (a public item of a workflow type).
    pub stage: bool,
    /// The actor (an admin) may publish without the approval, with a reason.
    pub r#override: bool,
}

/// What `actor` may do with `item` as it is now (role, token scopes, status and the
/// workflow state `wf`). A `true` flag means the method passes its permission,
/// transition and approval checks; it can still fail on a stale version, a schedule
/// time in the past or (delete) children.
pub fn allowed_actions(actor: &Actor, item: &ContentItem, wf: &WorkflowState) -> AllowedActions {
    let may = |action: Action| legal(action, item.status) && authorize(actor, item, action).is_ok();
    let public = matches!(
        item.status,
        ContentStatus::Published | ContentStatus::Scheduled | ContentStatus::Private
    );
    let edit_rights = editable(item.status) && can_edit(actor, item).is_ok();
    let approved = wf.approved();
    let publish = may(Action::Publish);
    let schedule = may(Action::Schedule(Timestamp::now()));
    let in_review = wf.review.is_some();
    let resubmit = wf.workflow
        && !in_review
        && item.status == ContentStatus::Pending
        && authorize(actor, item, Action::Submit).is_ok();
    AllowedActions {
        edit: edit_rights && (!wf.workflow || !public || wf.approver),
        publish: publish && approved,
        unpublish: may(Action::Unpublish),
        schedule: schedule && approved,
        submit: (may(Action::Submit) && !in_review) || resubmit,
        trash: may(Action::Trash),
        restore: may(Action::Restore),
        delete: deletable(item.status) && authorize_ownership(actor, item).is_ok(),
        trash_undoable: may(Action::Trash) && authorize(actor, item, Action::Restore).is_ok(),
        needs_approval: (publish || schedule) && !approved,
        stage: wf.workflow && public && edit_rights,
        r#override: (publish || schedule) && !approved && actor.role == Role::Admin,
    }
}
