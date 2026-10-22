//! Who is acting, and what their role allows (07 §1).

use ferrite_domain::{ContentItem, ContentStatus, Role, UserId};

use crate::CoreError;

/// The user a request acts for, with the role captured at request time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Actor {
    pub user: UserId,
    pub role: Role,
}

/// A permission checked by core services.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Capability {
    ReadAdmin,
    /// Contributors hold it only while the item is draft or pending (see [`can_edit`]).
    EditOwn,
    PublishOwn,
    EditOthers,
    PublishOthers,
    UploadMedia,
    ManageTerms,
    ManageMenus,
    ModerateComments,
    ManageUsers,
    ManageSettings,
    ManageThemes,
    ManagePlugins,
    Import,
    ManageRedirects,
}

impl Capability {
    /// The capability's name as in 07 §1, e.g. `edit_own`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadAdmin => "read_admin",
            Self::EditOwn => "edit_own",
            Self::PublishOwn => "publish_own",
            Self::EditOthers => "edit_others",
            Self::PublishOthers => "publish_others",
            Self::UploadMedia => "upload_media",
            Self::ManageTerms => "manage_terms",
            Self::ManageMenus => "manage_menus",
            Self::ModerateComments => "moderate_comments",
            Self::ManageUsers => "manage_users",
            Self::ManageSettings => "manage_settings",
            Self::ManageThemes => "manage_themes",
            Self::ManagePlugins => "manage_plugins",
            Self::Import => "import",
            Self::ManageRedirects => "manage_redirects",
        }
    }
}

/// Whether `role` holds `cap` at all. Status-dependent limits (contributors editing
/// only draft/pending items) are applied by [`can_edit`].
pub fn can(role: Role, cap: Capability) -> bool {
    use Capability::*;
    match cap {
        ReadAdmin | EditOwn => true,
        PublishOwn | UploadMedia => matches!(role, Role::Admin | Role::Editor | Role::Author),
        EditOthers | PublishOthers | ManageTerms | ManageMenus | ModerateComments => {
            matches!(role, Role::Admin | Role::Editor)
        }
        ManageUsers | ManageSettings | ManageThemes | ManagePlugins | Import | ManageRedirects => {
            role == Role::Admin
        }
    }
}

/// `Ok` if `actor` holds `cap`, else `Forbidden(cap name)`.
pub fn require(actor: &Actor, cap: Capability) -> Result<(), CoreError> {
    if can(actor.role, cap) {
        Ok(())
    } else {
        Err(CoreError::Forbidden(cap.as_str()))
    }
}

/// Whether `actor` may edit the existing `item`: own items need `edit_own` (contributors
/// only while the item is draft or pending), other people's need `edit_others`.
pub fn can_edit(actor: &Actor, item: &ContentItem) -> Result<(), CoreError> {
    if item.author_id != actor.user {
        return require(actor, Capability::EditOthers);
    }
    require(actor, Capability::EditOwn)?;
    if actor.role == Role::Contributor
        && !matches!(item.status, ContentStatus::Draft | ContentStatus::Pending)
    {
        return Err(CoreError::Forbidden(Capability::EditOwn.as_str()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use Capability::*;

    const ALL: [Capability; 15] = [
        ReadAdmin,
        EditOwn,
        PublishOwn,
        EditOthers,
        PublishOthers,
        UploadMedia,
        ManageTerms,
        ManageMenus,
        ModerateComments,
        ManageUsers,
        ManageSettings,
        ManageThemes,
        ManagePlugins,
        Import,
        ManageRedirects,
    ];

    fn granted(role: Role) -> Vec<&'static str> {
        ALL.iter()
            .filter(|c| can(role, **c))
            .map(|c| c.as_str())
            .collect()
    }

    #[test]
    fn capability_table() {
        assert_eq!(granted(Role::Admin).len(), ALL.len());
        assert_eq!(
            granted(Role::Editor),
            [
                "read_admin",
                "edit_own",
                "publish_own",
                "edit_others",
                "publish_others",
                "upload_media",
                "manage_terms",
                "manage_menus",
                "moderate_comments"
            ]
        );
        assert_eq!(
            granted(Role::Author),
            ["read_admin", "edit_own", "publish_own", "upload_media"]
        );
        assert_eq!(granted(Role::Contributor), ["read_admin", "edit_own"]);
    }

    #[test]
    fn require_names_the_capability() {
        let actor = Actor {
            user: UserId::new(),
            role: Role::Author,
        };
        assert!(require(&actor, PublishOwn).is_ok());
        assert!(matches!(
            require(&actor, ManageRedirects),
            Err(CoreError::Forbidden("manage_redirects"))
        ));
    }
}
