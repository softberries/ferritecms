//! The objects a captured write touches (design 17 §4) and the ops it records (§3.3).

use ferrite_domain::{CommentId, ContentId, RedirectId, TermId};

use crate::Area;

/// One object a change records: what [`snapshot`](super::snapshot::snapshot) reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ObjectRef {
    Content(ContentId),
    Media(ContentId),
    Term(TermId),
    /// A menu by name (M6-1: a translation is a menu of its own name, its locale in the
    /// snapshot).
    Menu(String),
    /// A content type by name.
    Type(String),
    Redirect(RedirectId),
    Comment(CommentId),
    /// A site option by key (settings, theme and plugin settings).
    Option(String),
    /// The active theme (`object_id` "active").
    ActiveTheme,
    /// An installed plugin by name.
    Plugin(String),
}

impl ObjectRef {
    /// The stored `object_kind`.
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::Content(_) => "content",
            Self::Media(_) => "media",
            Self::Term(_) => "term",
            Self::Menu(_) => "menu",
            Self::Type(_) => "type",
            Self::Redirect(_) => "redirect",
            Self::Comment(_) => "comment",
            Self::Option(_) => "option",
            Self::ActiveTheme => "theme",
            Self::Plugin(_) => "plugin",
        }
    }

    /// The stored `object_id`.
    pub(crate) fn id(&self) -> String {
        match self {
            Self::Content(id) | Self::Media(id) => id.to_string(),
            Self::Term(id) => id.to_string(),
            Self::Menu(name) | Self::Type(name) | Self::Option(name) | Self::Plugin(name) => {
                name.clone()
            }
            Self::Redirect(id) => id.to_string(),
            Self::Comment(id) => id.to_string(),
            Self::ActiveTheme => "active".into(),
        }
    }

    /// Parses a stored (`object_kind`, `object_id`) pair back; `None` for an unknown kind
    /// or a malformed id.
    pub(crate) fn parse(kind: &str, id: &str) -> Option<Self> {
        Some(match kind {
            "content" => Self::Content(id.parse().ok()?),
            "media" => Self::Media(id.parse().ok()?),
            "term" => Self::Term(id.parse().ok()?),
            "menu" => Self::Menu(id.to_owned()),
            "type" => Self::Type(id.to_owned()),
            "redirect" => Self::Redirect(id.parse().ok()?),
            "comment" => Self::Comment(id.parse().ok()?),
            "option" => Self::Option(id.to_owned()),
            "theme" if id == "active" => Self::ActiveTheme,
            "plugin" => Self::Plugin(id.to_owned()),
            _ => return None,
        })
    }

    /// The area whose read right a viewer needs to see the images (design 17 §8).
    pub(crate) fn area(&self) -> Area {
        match self {
            Self::Content(_) | Self::Type(_) => Area::Content,
            Self::Media(_) => Area::Media,
            Self::Term(_) => Area::Terms,
            Self::Menu(_) => Area::Menus,
            Self::Redirect(_) => Area::Redirects,
            Self::Comment(_) => Area::Comments,
            Self::Option(_) => Area::Settings,
            Self::ActiveTheme | Self::Plugin(_) => Area::Extensions,
        }
    }
}

/// What a captured write did (the stored `op`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Op {
    ContentCreate,
    ContentTranslate,
    ContentUpdate,
    ContentPublish,
    ContentUnpublish,
    ContentSchedule,
    ContentSubmit,
    ContentTrash,
    ContentRestore,
    ContentRestoreRevision,
    MediaUpload,
    MediaUpdate,
    MediaTrash,
    MediaRestore,
    TermCreate,
    TermUpdate,
    TermDelete,
    MenuPut,
    TypeCreate,
    TypeUpdate,
    RedirectCreate,
    RedirectUpdate,
    RedirectDelete,
    CommentModerate,
    CommentCreate,
    SettingsUpdate,
    ThemeActivate,
    ThemeSettings,
    PluginEnable,
    PluginDisable,
    PluginSettings,
}

impl Op {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ContentCreate => "content.create",
            Self::ContentTranslate => "content.translate",
            Self::ContentUpdate => "content.update",
            Self::ContentPublish => "content.publish",
            Self::ContentUnpublish => "content.unpublish",
            Self::ContentSchedule => "content.schedule",
            Self::ContentSubmit => "content.submit",
            Self::ContentTrash => "content.trash",
            Self::ContentRestore => "content.restore",
            Self::ContentRestoreRevision => "content.restore_revision",
            Self::MediaUpload => "media.upload",
            Self::MediaUpdate => "media.update",
            Self::MediaTrash => "media.trash",
            Self::MediaRestore => "media.restore",
            Self::TermCreate => "term.create",
            Self::TermUpdate => "term.update",
            Self::TermDelete => "term.delete",
            Self::MenuPut => "menu.put",
            Self::TypeCreate => "type.create",
            Self::TypeUpdate => "type.update",
            Self::RedirectCreate => "redirect.create",
            Self::RedirectUpdate => "redirect.update",
            Self::RedirectDelete => "redirect.delete",
            Self::CommentModerate => "comment.moderate",
            Self::CommentCreate => "comment.create",
            Self::SettingsUpdate => "settings.update",
            Self::ThemeActivate => "theme.activate",
            Self::ThemeSettings => "theme.settings",
            Self::PluginEnable => "plugin.enable",
            Self::PluginDisable => "plugin.disable",
            Self::PluginSettings => "plugin.settings",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refs_parse_back() {
        for r in [
            ObjectRef::Content(ContentId::new()),
            ObjectRef::Media(ContentId::new()),
            ObjectRef::Term(TermId::new()),
            ObjectRef::Menu("main".into()),
            ObjectRef::Type("book".into()),
            ObjectRef::Redirect(RedirectId::new()),
            ObjectRef::Comment(CommentId::new()),
            ObjectRef::Option("site.title".into()),
            ObjectRef::ActiveTheme,
            ObjectRef::Plugin("seo".into()),
        ] {
            assert_eq!(ObjectRef::parse(r.kind(), &r.id()), Some(r));
        }
        assert_eq!(ObjectRef::parse("theme", "x"), None);
        assert_eq!(ObjectRef::parse("content", "nope"), None);
        assert_eq!(ObjectRef::parse("nope", "x"), None);
    }
}
