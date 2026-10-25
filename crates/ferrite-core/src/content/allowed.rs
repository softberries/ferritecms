//! What an actor may do with one item right now ([`allowed_actions`], M2b P13): the
//! admin shows exactly the actions the server would accept, and never decides on its
//! own.
//!
//! Each flag is the conjunction of the permission check the matching method makes and
//! the transition being legal from the item's status, computed with the same
//! predicates the lifecycle and `update` call ([`authorize`], [`authorize_ownership`],
//! [`can_edit`], [`legal`], [`editable`], [`deletable`]), so the two cannot drift.

use ferrite_domain::{ContentItem, Timestamp};

use super::lifecycle::{Action, authorize, authorize_ownership, deletable, editable, legal};
use crate::{Actor, actor::can_edit};

/// The actions `actor` may take on an item ([`allowed_actions`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AllowedActions {
    /// `update` (and revision restore): edit rights and not trashed.
    pub edit: bool,
    /// `publish` (also "publish now" of a scheduled item).
    pub publish: bool,
    pub unpublish: bool,
    /// `schedule` (for a future time).
    pub schedule: bool,
    /// `submit` for review.
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
}

/// What `actor` may do with `item` as it is now (role, token scopes and status). A
/// `true` flag means the method passes its permission and transition checks; it can
/// still fail on a stale version, a schedule time in the past or (delete) children.
pub fn allowed_actions(actor: &Actor, item: &ContentItem) -> AllowedActions {
    let may = |action: Action| legal(action, item.status) && authorize(actor, item, action).is_ok();
    AllowedActions {
        edit: editable(item.status) && can_edit(actor, item).is_ok(),
        publish: may(Action::Publish),
        unpublish: may(Action::Unpublish),
        // The time does not change legality or permission.
        schedule: may(Action::Schedule(Timestamp::now())),
        submit: may(Action::Submit),
        trash: may(Action::Trash),
        restore: may(Action::Restore),
        delete: deletable(item.status) && authorize_ownership(actor, item).is_ok(),
        trash_undoable: may(Action::Trash) && authorize(actor, item, Action::Restore).is_ok(),
    }
}
