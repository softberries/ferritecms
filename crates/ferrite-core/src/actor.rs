//! Who is acting, what their role allows (07 §1), and which token scopes narrow it (P6).

use ferrite_domain::{ContentItem, ContentStatus, Role, UserId};

use crate::CoreError;

/// The user a request acts for, with the role captured at request time and the scopes of
/// the credential used. Sessions hold every scope; API tokens hold the scopes they were
/// created with. A capability needs both the role and the scope ([`require`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Actor {
    pub user: UserId,
    pub role: Role,
    pub scopes: Scopes,
}

impl Actor {
    /// An actor holding every scope (a session, or a system test fixture).
    pub fn new(user: UserId, role: Role) -> Self {
        Self {
            user,
            role,
            scopes: Scopes::ALL,
        }
    }

    /// The same actor narrowed (or widened) to `scopes`.
    pub fn with_scopes(self, scopes: Scopes) -> Self {
        Self { scopes, ..self }
    }
}

/// The part of the site a token scope covers (P6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Area {
    Content,
    Media,
    Terms,
    Menus,
    Comments,
    Users,
    Settings,
    Redirects,
    Extensions,
    Import,
    System,
}

impl Area {
    /// Every area, in scope-bit order.
    pub const ALL: [Area; 11] = [
        Area::Content,
        Area::Media,
        Area::Terms,
        Area::Menus,
        Area::Comments,
        Area::Users,
        Area::Settings,
        Area::Redirects,
        Area::Extensions,
        Area::Import,
        Area::System,
    ];

    fn index(self) -> u32 {
        self as u32
    }
}

/// Read or write access to an area; `Write` implies `Read`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Access {
    Read,
    Write,
}

/// One token scope, `<area>:<read|write>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Scope {
    pub area: Area,
    pub access: Access,
}

/// Scope names indexed by bit (`area * 2 + access`).
const SCOPE_NAMES: [&str; 22] = [
    "content:read",
    "content:write",
    "media:read",
    "media:write",
    "terms:read",
    "terms:write",
    "menus:read",
    "menus:write",
    "comments:read",
    "comments:write",
    "users:read",
    "users:write",
    "settings:read",
    "settings:write",
    "redirects:read",
    "redirects:write",
    "extensions:read",
    "extensions:write",
    "import:read",
    "import:write",
    "system:read",
    "system:write",
];

impl Scope {
    pub const fn read(area: Area) -> Self {
        Self {
            area,
            access: Access::Read,
        }
    }

    pub const fn write(area: Area) -> Self {
        Self {
            area,
            access: Access::Write,
        }
    }

    fn bit(self) -> u32 {
        self.area.index() * 2
            + match self.access {
                Access::Read => 0,
                Access::Write => 1,
            }
    }

    /// The scope's name, e.g. `content:read`.
    pub fn as_str(self) -> &'static str {
        SCOPE_NAMES
            .get(self.bit() as usize)
            .copied()
            .unwrap_or_default()
    }

    /// Parses a scope name exactly as [`as_str`](Self::as_str) writes it.
    pub fn parse(s: &str) -> Result<Scope, CoreError> {
        Area::ALL
            .iter()
            .flat_map(|&area| [Scope::read(area), Scope::write(area)])
            .find(|scope| scope.as_str() == s)
            .ok_or_else(|| CoreError::Invalid(format!("unknown scope {s:?}")))
    }
}

/// A set of [`Scope`]s (a bit set).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Scopes(u32);

impl Scopes {
    /// Every scope (sessions).
    pub const ALL: Scopes = Scopes((1 << SCOPE_NAMES.len()) - 1);
    /// No scope at all.
    pub const NONE: Scopes = Scopes(0);

    /// Parses a list of scope names; any unknown name is `Invalid`.
    pub fn from_strs(names: &[String]) -> Result<Scopes, CoreError> {
        names.iter().try_fold(Scopes::NONE, |acc, name| {
            Ok(Scopes(acc.0 | 1 << Scope::parse(name)?.bit()))
        })
    }

    /// Whether the set grants `scope`; a held `Write` grants `Read` of the same area.
    pub fn allows(self, scope: Scope) -> bool {
        let has = |s: Scope| self.0 & (1 << s.bit()) != 0;
        match scope.access {
            Access::Read => has(scope) || has(Scope::write(scope.area)),
            Access::Write => has(scope),
        }
    }

    /// The names of the scopes held, in a fixed (area, read-then-write) order.
    pub fn to_strings(self) -> Vec<String> {
        SCOPE_NAMES
            .iter()
            .enumerate()
            .filter(|(bit, _)| self.0 & (1 << bit) != 0)
            .map(|(_, name)| (*name).to_owned())
            .collect()
    }
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
    /// Every capability, in the order of 07 §1.
    pub const ALL: [Capability; 15] = [
        Self::ReadAdmin,
        Self::EditOwn,
        Self::PublishOwn,
        Self::EditOthers,
        Self::PublishOthers,
        Self::UploadMedia,
        Self::ManageTerms,
        Self::ManageMenus,
        Self::ModerateComments,
        Self::ManageUsers,
        Self::ManageSettings,
        Self::ManageThemes,
        Self::ManagePlugins,
        Self::Import,
        Self::ManageRedirects,
    ];

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

    /// The token scope the capability also needs; `None` for `read_admin`, which every
    /// credential holds.
    pub fn scope(self) -> Option<Scope> {
        use Capability::*;
        let area = match self {
            ReadAdmin => return None,
            EditOwn | PublishOwn | EditOthers | PublishOthers => Area::Content,
            UploadMedia => Area::Media,
            ManageTerms => Area::Terms,
            ManageMenus => Area::Menus,
            ModerateComments => Area::Comments,
            ManageUsers => Area::Users,
            ManageSettings => Area::Settings,
            ManageThemes | ManagePlugins => Area::Extensions,
            Import => Area::Import,
            ManageRedirects => Area::Redirects,
        };
        Some(Scope::write(area))
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

/// `Ok` if `actor`'s role holds `cap` (else `Forbidden(cap name)`) and its scopes allow
/// the capability's scope (else `Forbidden(scope name)`).
pub fn require(actor: &Actor, cap: Capability) -> Result<(), CoreError> {
    if !can(actor.role, cap) {
        return Err(CoreError::Forbidden(cap.as_str()));
    }
    require_scope(actor, cap.scope())
}

/// `Ok` if `actor` may read `area` in the admin: `read_admin`, then the `<area>:read`
/// scope (held directly or through `<area>:write`).
pub fn require_read(actor: &Actor, area: Area) -> Result<(), CoreError> {
    require(actor, Capability::ReadAdmin)?;
    require_scope(actor, Some(Scope::read(area)))
}

/// `Ok` if `actor`'s scopes allow `scope` (or no scope is needed).
pub(crate) fn require_scope(actor: &Actor, scope: Option<Scope>) -> Result<(), CoreError> {
    match scope {
        Some(s) if !actor.scopes.allows(s) => Err(CoreError::Forbidden(s.as_str())),
        _ => Ok(()),
    }
}

/// Whether `actor` may read `item` in the admin (after [`require_read`]`(Content)`):
/// published items are readable by everyone, own items in any status by their author,
/// and everything else (others' drafts, pending, scheduled, private and trashed items)
/// needs a role holding `edit_others` (editor, admin). There is no separate
/// `read_private` capability: WordPress's `read_private_posts` is granted to the same
/// roles as `edit_others_posts`, so `edit_others` stands for it. Only the role is
/// checked, not a write scope: an admin's `content:read` token reads every item.
pub fn can_read(actor: &Actor, item: &ContentItem) -> bool {
    item.status == ContentStatus::Published
        || item.author_id == actor.user
        || can(actor.role, Capability::EditOthers)
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

    fn granted(role: Role) -> Vec<&'static str> {
        Capability::ALL
            .iter()
            .filter(|c| can(role, **c))
            .map(|c| c.as_str())
            .collect()
    }

    #[test]
    fn capability_table() {
        assert_eq!(granted(Role::Admin).len(), Capability::ALL.len());
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
    fn scopes_narrow_capabilities() {
        let read_content = Scopes::from_strs(&["content:read".to_owned()]).unwrap();
        let admin = Actor::new(UserId::new(), Role::Admin).with_scopes(read_content);
        assert!(require_read(&admin, Area::Content).is_ok());
        assert!(matches!(
            require(&admin, EditOwn),
            Err(CoreError::Forbidden("content:write"))
        ));
        assert!(matches!(
            require(&admin, PublishOthers),
            Err(CoreError::Forbidden("content:write"))
        ));
        assert!(matches!(
            require(&admin, ManageSettings),
            Err(CoreError::Forbidden("settings:write"))
        ));
        assert!(matches!(
            require_read(&admin, Area::Settings),
            Err(CoreError::Forbidden("settings:read"))
        ));
        // ReadAdmin carries no scope: any token may reach the admin at all.
        assert!(require(&admin, ReadAdmin).is_ok());

        let contributor = Actor::new(UserId::new(), Role::Contributor);
        assert_eq!(contributor.scopes, Scopes::ALL);
        assert!(matches!(
            require(&contributor, PublishOwn),
            Err(CoreError::Forbidden("publish_own"))
        ));
        assert!(require(&contributor, EditOwn).is_ok());

        // Write implies read; read does not imply write.
        let write = Scopes::from_strs(&["media:write".to_owned()]).unwrap();
        assert!(write.allows(Scope::read(Area::Media)));
        assert!(write.allows(Scope::write(Area::Media)));
        assert!(!write.allows(Scope::read(Area::Content)));
        let read = Scopes::from_strs(&["media:read".to_owned()]).unwrap();
        assert!(!read.allows(Scope::write(Area::Media)));
        assert!(!Scopes::NONE.allows(Scope::read(Area::System)));

        // Every scope name parses back, and the set round-trips through strings.
        let mut names = Vec::new();
        for area in Area::ALL {
            for access in [Access::Read, Access::Write] {
                let scope = Scope { area, access };
                assert_eq!(Scope::parse(scope.as_str()).unwrap(), scope);
                assert!(Scopes::ALL.allows(scope));
                names.push(scope.as_str().to_owned());
            }
        }
        assert_eq!(names.len(), 22);
        assert_eq!(
            names
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            22
        );
        assert_eq!(Scopes::from_strs(&names).unwrap(), Scopes::ALL);
        assert_eq!(Scopes::ALL.to_strings(), names);
        assert_eq!(
            Scopes::from_strs(&["users:write".into(), "content:read".into()])
                .unwrap()
                .to_strings(),
            ["content:read", "users:write"]
        );
        for bad in [
            "",
            "content",
            "content:admin",
            "nope:read",
            "Content:read",
            "content:read ",
        ] {
            assert!(
                matches!(Scope::parse(bad), Err(CoreError::Invalid(_))),
                "{bad:?}"
            );
        }
        assert!(Scopes::from_strs(&["content:read".into(), "x".into()]).is_err());
    }

    #[test]
    fn capability_scopes() {
        let expect = [
            (ReadAdmin, None),
            (EditOwn, Some("content:write")),
            (PublishOwn, Some("content:write")),
            (EditOthers, Some("content:write")),
            (PublishOthers, Some("content:write")),
            (UploadMedia, Some("media:write")),
            (ManageTerms, Some("terms:write")),
            (ManageMenus, Some("menus:write")),
            (ModerateComments, Some("comments:write")),
            (ManageUsers, Some("users:write")),
            (ManageSettings, Some("settings:write")),
            (ManageThemes, Some("extensions:write")),
            (ManagePlugins, Some("extensions:write")),
            (Import, Some("import:write")),
            (ManageRedirects, Some("redirects:write")),
        ];
        assert_eq!(expect.len(), Capability::ALL.len());
        for (cap, scope) in expect {
            assert_eq!(cap.scope().map(Scope::as_str), scope, "{cap:?}");
        }
    }

    #[test]
    fn require_names_the_capability() {
        let actor = Actor::new(UserId::new(), Role::Author);
        assert!(require(&actor, PublishOwn).is_ok());
        assert!(matches!(
            require(&actor, ManageRedirects),
            Err(CoreError::Forbidden("manage_redirects"))
        ));
    }
}
