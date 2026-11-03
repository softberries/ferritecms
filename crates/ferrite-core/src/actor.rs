//! Who is acting, what their role allows (07 §1), and which token scopes narrow it (P6).

use ferrite_domain::{
    ApiTokenId, ContentItem, ContentStatus, OAuthGrantId, Role, Timestamp, UserId,
};

use crate::CoreError;

/// The user a request acts for, with the role captured at request time and the scopes of
/// the credential used. Sessions hold every scope; API tokens hold the scopes they were
/// created with. A capability needs both the role and the scope ([`require`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Actor {
    pub user: UserId,
    pub role: Role,
    pub scopes: Scopes,
    /// The AI assistant connection acting, set only by the MCP guard (design 17 §3.1):
    /// its writes are recorded as change sets. `None` for sessions, REST tokens, the CLI
    /// and jobs.
    pub agent: Option<Agent>,
}

/// An assistant's connection over MCP: an OAuth grant (stable across token rotation) or
/// a personal API token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Agent {
    OAuth(OAuthGrantId),
    Token(ApiTokenId),
}

impl Actor {
    /// An actor holding every scope (a session, or a system test fixture), no agent.
    pub fn new(user: UserId, role: Role) -> Self {
        Self {
            user,
            role,
            scopes: Scopes::ALL,
            agent: None,
        }
    }

    /// The same actor narrowed (or widened) to `scopes`; the agent stays.
    pub fn with_scopes(self, scopes: Scopes) -> Self {
        Self { scopes, ..self }
    }

    /// The same actor marked as acting through `agent` (the MCP guard only).
    pub fn with_agent(self, agent: Agent) -> Self {
        Self {
            agent: Some(agent),
            ..self
        }
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
    /// Outgoing webhooks (M6-6, design 18 §6.2), after `system` so older bits stay.
    Webhooks,
}

impl Area {
    /// Every area, in scope-bit order.
    pub const ALL: [Area; 12] = [
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
        Area::Webhooks,
    ];

    const fn index(self) -> u32 {
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
const SCOPE_NAMES: [&str; 24] = [
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
    "webhooks:read",
    "webhooks:write",
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

    const fn bit(self) -> u32 {
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
    /// What an OAuth client can be granted (design 12 §3.3): every scope except the
    /// writes no MCP tool uses, `users:write`, `import:write` and `system:write` (their
    /// reads stay), so a grant cannot be stretched beyond what assistants do, and both
    /// `webhooks:*` scopes (design 18 §9: a webhook is an exfiltration channel).
    pub const OAUTH: Scopes = Scopes(
        Scopes::ALL.0
            & !(1 << Scope::write(Area::Users).bit()
                | 1 << Scope::write(Area::Import).bit()
                | 1 << Scope::write(Area::System).bit()
                | 1 << Scope::read(Area::Webhooks).bit()
                | 1 << Scope::write(Area::Webhooks).bit()),
    );

    /// The set holding `scope` alone.
    pub(crate) fn only(scope: Scope) -> Scopes {
        Scopes(1 << scope.bit())
    }

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

    /// The scopes held, in a fixed (area, read-then-write) order.
    pub fn iter(self) -> impl Iterator<Item = Scope> {
        Area::ALL
            .into_iter()
            .flat_map(|area| [Scope::read(area), Scope::write(area)])
            .filter(move |s| self.0 & (1 << s.bit()) != 0)
    }

    /// Whether every scope of `other` is allowed by this set (a held `Write` covers the
    /// `Read` of its area).
    pub fn covers(self, other: Scopes) -> bool {
        other.iter().all(|s| self.allows(s))
    }

    /// Per area, the strongest access both this set and `limit` allow: `Write` when both
    /// allow it, else `Read` when both allow that, else nothing. A requested
    /// `<area>:write` that `limit` only allows reading becomes `<area>:read`.
    pub fn within(self, limit: Scopes) -> Scopes {
        Area::ALL.iter().fold(Scopes::NONE, |acc, &area| {
            match [Scope::write(area), Scope::read(area)]
                .into_iter()
                .find(|&s| self.allows(s) && limit.allows(s))
            {
                Some(s) => Scopes(acc.0 | Scopes::only(s).0),
                None => acc,
            }
        })
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
    /// System status, nodes, audit and jobs (M2a-2b P15).
    ManageSystem,
    /// Outgoing webhooks (M6-6, design 18 §6.2): administrators only.
    ManageWebhooks,
}

impl Capability {
    /// Every capability, in the order of 07 §1.
    pub const ALL: [Capability; 17] = [
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
        Self::ManageSystem,
        Self::ManageWebhooks,
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
            Self::ManageSystem => "manage_system",
            Self::ManageWebhooks => "manage_webhooks",
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
            ManageSystem => Area::System,
            ManageWebhooks => Area::Webhooks,
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
        ManageUsers | ManageSettings | ManageThemes | ManagePlugins | Import | ManageRedirects
        | ManageSystem | ManageWebhooks => role == Role::Admin,
    }
}

/// The scopes `role` can use (design 12 §3.3, the grantable scopes of an OAuth consent):
/// `<area>:write` when the role holds a capability that needs it ([`Capability::scope`]),
/// and `<area>:read` when it can write the area or when the area's reads need only
/// `read_admin` ([`require_read`]: content, media, terms, menus, users, settings and
/// redirects). Comments, extensions, import and system are read through
/// [`require_reader`], so their reads need the role's capability. Every role gets
/// `content:write` (`edit_own`); `users:write` needs `manage_users`, so only an
/// administrator can grant the account-management scope.
pub fn role_scopes(role: Role) -> Scopes {
    const READ_FOR_EVERY_ROLE: [Area; 7] = [
        Area::Content,
        Area::Media,
        Area::Terms,
        Area::Menus,
        Area::Users,
        Area::Settings,
        Area::Redirects,
    ];
    let writes = Capability::ALL
        .iter()
        .filter(|&&cap| can(role, cap))
        .filter_map(|cap| cap.scope())
        .flat_map(|s| [Scope::read(s.area), s]);
    let reads = READ_FOR_EVERY_ROLE.into_iter().map(Scope::read);
    writes
        .chain(reads)
        .fold(Scopes::NONE, |acc, s| Scopes(acc.0 | Scopes::only(s).0))
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

/// `Ok` if `actor` may read what `cap` manages (extension reads, M2a-2 P10): the role
/// holds `cap` (else `Forbidden(cap name)`), then [`require_read`] of the capability's
/// area (else `Forbidden("<area>:read")`). A capability without a scope needs only
/// `read_admin`.
pub fn require_reader(actor: &Actor, cap: Capability) -> Result<(), CoreError> {
    if !can(actor.role, cap) {
        return Err(CoreError::Forbidden(cap.as_str()));
    }
    match cap.scope() {
        Some(scope) => require_read(actor, scope.area),
        None => require(actor, Capability::ReadAdmin),
    }
}

/// `Ok` if `actor`'s scopes allow `scope` (or no scope is needed).
pub(crate) fn require_scope(actor: &Actor, scope: Option<Scope>) -> Result<(), CoreError> {
    match scope {
        Some(s) if !actor.scopes.allows(s) => Err(CoreError::Forbidden(s.as_str())),
        _ => Ok(()),
    }
}

/// Whether `actor` may read `item` in the admin (after [`require_read`]`(Content)`) at
/// `now`: items publicly visible at `now` (the same predicate as the public site,
/// `is_public`: status published and `published_at <= now`)
/// are readable by everyone, own items in any status by their author, and everything
/// else (others' drafts, pending, scheduled, private, trashed and future-dated
/// published items) needs a role holding `edit_others` (editor, admin). The SQL mirror
/// is `ContentQuery::readable_by` / `SearchQuery::readable_by`. There is no separate
/// `read_private` capability: WordPress's `read_private_posts` is granted to the same
/// roles as `edit_others_posts`, so `edit_others` stands for it. Only the role is
/// checked, not a write scope: an admin's `content:read` token reads every item.
pub fn can_read(actor: &Actor, item: &ContentItem, now: Timestamp) -> bool {
    crate::site::is_public(item, now)
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

/// Whether `actor`'s role may edit `item`: the role half of [`can_edit`], without the
/// `content:write` scope check. Revisions are read with this (R26): their history can
/// hold text that was removed, so only someone who could edit the item reads it (as
/// WordPress requires `edit_post` on the parent), while a read-only token of such a
/// role still may.
pub fn role_can_edit(actor: &Actor, item: &ContentItem) -> bool {
    if item.author_id != actor.user {
        return can(actor.role, Capability::EditOthers);
    }
    can(actor.role, Capability::EditOwn)
        && (actor.role != Role::Contributor
            || matches!(item.status, ContentStatus::Draft | ContentStatus::Pending))
}

#[cfg(test)]
mod tests {
    use super::*;
    use Capability::*;

    fn item(author_id: UserId, status: ContentStatus) -> ContentItem {
        let now = Timestamp::now();
        ContentItem {
            id: ferrite_domain::ContentId::new(),
            kind: "post".into(),
            status,
            slug: "s".into(),
            parent_id: None,
            author_id,
            title: String::new(),
            excerpt: String::new(),
            body: serde_json::json!({}),
            body_html: None,
            fields: serde_json::json!({}),
            menu_order: 0,
            published_at: None,
            scheduled_at: None,
            created_at: now,
            updated_at: now,
            version: 1,
            locale: "en".into(),
            translation_group: ferrite_domain::ContentId::new(),
        }
    }

    /// `role_can_edit` is `can_edit` for a full-scope actor, and ignores the scopes.
    #[test]
    fn role_can_edit_mirrors_can_edit() {
        use ContentStatus::*;
        let read_only = Scopes::from_strs(&["content:read".to_owned()]).unwrap();
        for role in [Role::Admin, Role::Editor, Role::Author, Role::Contributor] {
            let actor = Actor::new(UserId::new(), role);
            for author in [actor.user, UserId::new()] {
                for status in [Draft, Pending, Scheduled, Published, Private, Trash] {
                    let it = item(author, status);
                    let full = can_edit(&actor, &it).is_ok();
                    assert_eq!(role_can_edit(&actor, &it), full, "{role:?} {status:?}");
                    let reader = actor.with_scopes(read_only);
                    assert!(can_edit(&reader, &it).is_err());
                    assert_eq!(role_can_edit(&reader, &it), full, "{role:?} {status:?}");
                }
            }
        }
        let contributor = Actor::new(UserId::new(), Role::Contributor);
        assert!(role_can_edit(&contributor, &item(contributor.user, Draft)));
        assert!(!role_can_edit(
            &contributor,
            &item(contributor.user, Published)
        ));
        assert!(!role_can_edit(&contributor, &item(UserId::new(), Draft)));
    }

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
    fn role_scopes_follow_the_capability_table() {
        let names = |role| role_scopes(role).to_strings();
        assert_eq!(role_scopes(Role::Admin), Scopes::ALL);
        assert_eq!(
            names(Role::Contributor),
            [
                "content:read",
                "content:write",
                "media:read",
                "terms:read",
                "menus:read",
                "users:read",
                "settings:read",
                "redirects:read"
            ]
        );
        let author = role_scopes(Role::Author);
        assert!(author.allows(Scope::write(Area::Media)));
        assert!(!author.allows(Scope::read(Area::Comments)));
        let editor = role_scopes(Role::Editor);
        for area in [Area::Terms, Area::Menus, Area::Comments] {
            assert!(editor.allows(Scope::write(area)), "{area:?}");
        }
        for area in [Area::Extensions, Area::Import, Area::System] {
            assert!(!editor.allows(Scope::read(area)), "{area:?}");
        }
        assert!(!editor.allows(Scope::write(Area::Users)));
    }

    #[test]
    fn oauth_scopes_leave_out_unused_writes() {
        let oauth = Scopes::OAUTH.to_strings();
        assert_eq!(oauth.len(), 19);
        for gone in ["users:write", "import:write", "system:write"] {
            assert!(!oauth.iter().any(|s| s == gone), "{gone}");
        }
        for area in [Area::Users, Area::Import, Area::System] {
            assert!(Scopes::OAUTH.allows(Scope::read(area)), "{area:?}");
        }
        assert!(Scopes::OAUTH.allows(Scope::write(Area::Content)));
        assert_eq!(
            Scopes::ALL.within(Scopes::OAUTH).to_strings(),
            [
                "content:write",
                "media:write",
                "terms:write",
                "menus:write",
                "comments:write",
                "users:read",
                "settings:write",
                "redirects:write",
                "extensions:write",
                "import:read",
                "system:read"
            ]
        );
    }

    #[test]
    fn within_keeps_the_strongest_common_access() {
        let set = |names: &[&str]| {
            Scopes::from_strs(&names.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>()).unwrap()
        };
        let asked = set(&["content:write", "system:write", "media:read"]);
        let limit = set(&["content:write", "system:read", "terms:write"]);
        assert_eq!(
            asked.within(limit).to_strings(),
            ["content:write", "system:read"]
        );
        assert!(set(&["content:write"]).covers(set(&["content:read"])));
        assert!(!set(&["content:read"]).covers(set(&["content:write"])));
        assert!(Scopes::ALL.covers(Scopes::ALL));
        assert_eq!(Scopes::ALL.iter().count(), 24);
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
        // A reader needs the role's capability and the area's read scope (P10).
        let reader = Actor::new(UserId::new(), Role::Admin)
            .with_scopes(Scopes::from_strs(&["extensions:read".to_owned()]).unwrap());
        assert!(require_reader(&reader, ManagePlugins).is_ok());
        assert!(matches!(
            require(&reader, ManagePlugins),
            Err(CoreError::Forbidden("extensions:write"))
        ));
        assert!(matches!(
            require_reader(&admin, ManageThemes),
            Err(CoreError::Forbidden("extensions:read"))
        ));
        let editor = Actor::new(UserId::new(), Role::Editor);
        assert!(matches!(
            require_reader(&editor, ManagePlugins),
            Err(CoreError::Forbidden("manage_plugins"))
        ));
        assert!(require_reader(&editor, ReadAdmin).is_ok());
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
        assert_eq!(names.len(), 24);
        assert_eq!(
            names
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            24
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
            (ManageSystem, Some("system:write")),
            (ManageWebhooks, Some("webhooks:write")),
        ];
        assert_eq!(expect.len(), Capability::ALL.len());
        for (cap, scope) in expect {
            assert_eq!(cap.scope().map(Scope::as_str), scope, "{cap:?}");
        }
    }

    #[test]
    fn webhooks_are_admin_only_and_never_oauth() {
        for role in [Role::Editor, Role::Author, Role::Contributor] {
            assert!(!can(role, ManageWebhooks), "{role:?}");
        }
        assert!(can(Role::Admin, ManageWebhooks));
        assert!(!Scopes::OAUTH.allows(Scope::read(Area::Webhooks)));
        assert!(!Scopes::OAUTH.allows(Scope::write(Area::Webhooks)));
        // Appended after `system`: the older scope bits keep their positions.
        assert_eq!(Scope::write(Area::System).bit(), 21);
        assert_eq!(Scope::read(Area::Webhooks).as_str(), "webhooks:read");
        let system = Actor::new(UserId::new(), Role::Admin).with_scopes(
            Scopes::from_strs(&["system:write".into(), "settings:write".into()]).unwrap(),
        );
        assert!(matches!(
            require(&system, ManageWebhooks),
            Err(CoreError::Forbidden("webhooks:write"))
        ));
        assert!(matches!(
            require_reader(&system, ManageWebhooks),
            Err(CoreError::Forbidden("webhooks:read"))
        ));
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
