//! [`Head`]: the builder over the WIT `head` record that `render.head` filters take and
//! return (re-exported at the crate root as `ferrite_sdk::Head`).

use crate::bindings::ferrite::plugin::types as wit;
use crate::{LinkTag, MetaTag, ScriptTag};

/// Builder over the WIT `head` record.
#[derive(Debug, Clone)]
pub struct Head(wit::Head);

impl Default for Head {
    fn default() -> Self {
        Self(wit::Head {
            title: None,
            metas: Vec::new(),
            links: Vec::new(),
            scripts: Vec::new(),
        })
    }
}

impl Head {
    /// Add `<meta name=… content=…>`.
    pub fn meta(&mut self, name: &str, content: &str) -> &mut Self {
        self.0.metas.push(MetaTag {
            name: Some(name.to_owned()),
            property: None,
            content: content.to_owned(),
        });
        self
    }

    /// Add `<meta property=… content=…>` (Open Graph and friends).
    pub fn og(&mut self, property: &str, content: &str) -> &mut Self {
        self.0.metas.push(MetaTag {
            name: None,
            property: Some(property.to_owned()),
            content: content.to_owned(),
        });
        self
    }

    /// Add `<link rel=… href=…>`.
    pub fn link(&mut self, rel: &str, href: &str) -> &mut Self {
        self.0.links.push(LinkTag {
            rel: rel.to_owned(),
            href: href.to_owned(),
            hreflang: None,
            kind: None,
        });
        self
    }

    /// Add `<link rel=… href=… type=…>` (for example an RSS discovery link).
    pub fn link_typed(&mut self, rel: &str, href: &str, kind: &str) -> &mut Self {
        self.0.links.push(LinkTag {
            rel: rel.to_owned(),
            href: href.to_owned(),
            hreflang: None,
            kind: Some(kind.to_owned()),
        });
        self
    }

    /// Add a JSON-LD `<script type="application/ld+json">` block. `json` must be valid
    /// JSON; the host validates it and escapes `</script>` sequences.
    pub fn json_ld(&mut self, json: &str) -> &mut Self {
        self.0.scripts.push(ScriptTag {
            src: None,
            json_ld: Some(json.to_owned()),
        });
        self
    }

    /// True when a `<meta name=…>` with this name is present.
    pub fn has_meta_name(&self, name: &str) -> bool {
        self.0.metas.iter().any(|m| m.name.as_deref() == Some(name))
    }

    /// True when a `<meta property=…>` with this property is present.
    pub fn has_meta_property(&self, prop: &str) -> bool {
        self.meta_property(prop).is_some()
    }

    /// The content of the first `<meta property=…>` with this property.
    pub fn meta_property(&self, prop: &str) -> Option<&str> {
        self.0
            .metas
            .iter()
            .find(|m| m.property.as_deref() == Some(prop))
            .map(|m| m.content.as_str())
    }

    /// Set the document title.
    pub fn title(&mut self, title: &str) -> &mut Self {
        self.0.title = Some(title.to_owned());
        self
    }

    /// Borrow the underlying WIT record.
    pub fn as_wit(&self) -> &wit::Head {
        &self.0
    }
}

impl From<wit::Head> for Head {
    fn from(h: wit::Head) -> Self {
        Self(h)
    }
}

impl From<Head> for wit::Head {
    fn from(h: Head) -> Self {
        h.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_queries_tell_names_from_properties() {
        let mut h = Head::default();
        h.meta("description", "d").og("og:title", "T");
        assert!(h.has_meta_name("description"));
        assert!(!h.has_meta_name("og:title"));
        assert!(h.has_meta_property("og:title"));
        assert!(!h.has_meta_property("description"));
        assert!(!h.has_meta_property("og:url"));
        assert_eq!(h.meta_property("og:title"), Some("T"));
        assert_eq!(h.meta_property("og:url"), None);
    }

    #[test]
    fn meta_property_returns_the_first_match() {
        let mut h = Head::default();
        h.og("article:tag", "a").og("article:tag", "b");
        assert_eq!(h.meta_property("article:tag"), Some("a"));
    }

    #[test]
    fn queries_see_a_head_that_came_from_the_host() {
        let h: Head = wit::Head {
            title: None,
            metas: vec![MetaTag {
                name: Some("robots".into()),
                property: None,
                content: "noindex".into(),
            }],
            links: Vec::new(),
            scripts: Vec::new(),
        }
        .into();
        assert!(h.has_meta_name("robots"));
    }

    #[test]
    fn json_ld_pushes_a_script_tag_without_src() {
        let mut h = Head::default();
        h.json_ld(r#"{"@type":"WebSite"}"#);
        let scripts = &h.as_wit().scripts;
        assert_eq!(scripts.len(), 1);
        assert_eq!(scripts[0].src, None);
        assert_eq!(
            scripts[0].json_ld.as_deref(),
            Some(r#"{"@type":"WebSite"}"#)
        );
    }

    #[test]
    fn link_typed_sets_the_type() {
        let mut h = Head::default();
        h.link_typed("alternate", "https://x.test/feed/", "application/rss+xml");
        let l = &h.as_wit().links[0];
        assert_eq!(l.rel, "alternate");
        assert_eq!(l.href, "https://x.test/feed/");
        assert_eq!(l.kind.as_deref(), Some("application/rss+xml"));
        assert_eq!(l.hreflang, None);
    }

    #[test]
    fn plain_link_has_no_type() {
        let mut h = Head::default();
        h.link("canonical", "https://x.test/");
        assert_eq!(h.as_wit().links[0].kind, None);
    }
}
