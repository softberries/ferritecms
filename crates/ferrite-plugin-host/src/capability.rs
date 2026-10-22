//! Capability-to-interface mapping and the hook catalogue (design 05 §4).

use std::collections::BTreeSet;

use crate::manifest::{Capabilities, HtmlMode, MailScope};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterType {
    Head,
    Html,
    Text,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookKind {
    Filter(FilterType),
    Action,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeadlineClass {
    Render,
    Action,
    Route,
}

#[derive(Debug, Clone, Copy)]
pub struct HookSpec {
    pub name: &'static str,
    pub kind: HookKind,
    /// Whether the declared capabilities allow subscribing to this hook.
    pub requires: fn(&Capabilities) -> bool,
    pub deadline: DeadlineClass,
}

static CATALOGUE: [HookSpec; 8] = [
    HookSpec {
        name: "render.head",
        kind: HookKind::Filter(FilterType::Head),
        requires: |c| c.render_head,
        deadline: DeadlineClass::Render,
    },
    HookSpec {
        name: "content.body",
        kind: HookKind::Filter(FilterType::Html),
        requires: |c| c.render_html != HtmlMode::Off,
        deadline: DeadlineClass::Render,
    },
    HookSpec {
        name: "render.footer",
        kind: HookKind::Filter(FilterType::Html),
        requires: |c| c.render_html != HtmlMode::Off,
        deadline: DeadlineClass::Render,
    },
    HookSpec {
        name: "content.excerpt",
        kind: HookKind::Filter(FilterType::Text),
        requires: |c| c.render_text,
        deadline: DeadlineClass::Render,
    },
    HookSpec {
        name: "post.saved",
        kind: HookKind::Action,
        requires: |_| true,
        deadline: DeadlineClass::Action,
    },
    HookSpec {
        name: "post.published",
        kind: HookKind::Action,
        requires: |_| true,
        deadline: DeadlineClass::Action,
    },
    HookSpec {
        name: "user.created",
        kind: HookKind::Action,
        requires: |_| true,
        deadline: DeadlineClass::Action,
    },
    HookSpec {
        name: "request.served",
        kind: HookKind::Action,
        requires: |_| true,
        deadline: DeadlineClass::Action,
    },
];

/// All hooks of world `ferrite:plugin/plugin@0.1.0`.
pub fn hook_catalogue() -> &'static [HookSpec] {
    &CATALOGUE
}

/// Host-provided WIT interfaces a plugin can import.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Iface {
    Log,
    Content,
    Meta,
    Options,
    Kv,
    Http,
    Settings,
    Mail,
}

impl Iface {
    /// Fully qualified WIT interface name.
    pub fn wit_name(self) -> &'static str {
        match self {
            Iface::Log => "ferrite:plugin/log@0.1.0",
            Iface::Content => "ferrite:plugin/content@0.1.0",
            Iface::Meta => "ferrite:plugin/meta@0.1.0",
            Iface::Options => "ferrite:plugin/options@0.1.0",
            Iface::Kv => "ferrite:plugin/kv@0.1.0",
            Iface::Http => "ferrite:plugin/http@0.1.0",
            Iface::Settings => "ferrite:plugin/settings@0.1.0",
            Iface::Mail => "ferrite:plugin/mail@0.1.0",
        }
    }
}

/// Interfaces the linker may expose to a plugin with these capabilities.
/// `Log` and `Settings` are always granted.
pub fn granted_interfaces(c: &Capabilities) -> BTreeSet<Iface> {
    let mut set = BTreeSet::from([Iface::Log, Iface::Settings]);
    let conditional = [
        (Iface::Content, !c.content_read.is_empty()),
        (
            Iface::Meta,
            !c.content_meta_read.is_empty() || !c.content_meta_write.is_empty(),
        ),
        (Iface::Options, !c.options_read.is_empty()),
        (Iface::Kv, c.kv),
        (Iface::Http, !c.http.is_empty()),
        (Iface::Mail, c.mail == MailScope::Admins),
    ];
    set.extend(
        conditional
            .into_iter()
            .filter(|(_, on)| *on)
            .map(|(i, _)| i),
    );
    set
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::Manifest;
    use crate::manifest::tests::DESIGN_EXAMPLE;

    #[test]
    fn granted_interfaces_minimal() {
        let got = granted_interfaces(&Capabilities::default());
        assert_eq!(got, BTreeSet::from([Iface::Log, Iface::Settings]));
    }

    #[test]
    fn granted_interfaces_full() {
        let m = Manifest::parse(DESIGN_EXAMPLE).expect("design example parses");
        assert_eq!(
            granted_interfaces(&m.capabilities),
            BTreeSet::from([
                Iface::Log,
                Iface::Settings,
                Iface::Content,
                Iface::Meta,
                Iface::Options,
                Iface::Kv,
                Iface::Http,
                Iface::Mail
            ])
        );
    }

    #[test]
    fn catalogue_matches_design() {
        let names: Vec<_> = hook_catalogue().iter().map(|h| h.name).collect();
        assert_eq!(
            names,
            [
                "render.head",
                "content.body",
                "render.footer",
                "content.excerpt",
                "post.saved",
                "post.published",
                "user.created",
                "request.served"
            ]
        );
    }
}
