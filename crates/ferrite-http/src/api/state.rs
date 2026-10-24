//! What every API handler shares: the store and the core services, plus values derived
//! from `site.url` once at boot.

use std::sync::Arc;

use axum::http::Uri;
use ferrite_core::{
    AuthService, ContentService, ContentTypeService, ExtensionAdmin, MenuService, OptionService,
    RedirectService, TaxonomyService, UserService,
};
use ferrite_media::MediaStore;
use ferrite_store::Store;

use crate::extensions::Installer;

/// The inputs of [`ApiState::new`].
pub struct ApiParts {
    pub store: Arc<dyn Store>,
    pub auth: Arc<AuthService>,
    pub users: Arc<UserService>,
    pub content: Arc<ContentService>,
    pub types: Arc<ContentTypeService>,
    pub taxonomy: Arc<TaxonomyService>,
    pub menus: Arc<MenuService>,
    pub options: Arc<OptionService>,
    pub redirects: Arc<RedirectService>,
    pub extensions: Arc<ExtensionAdmin>,
    /// The node's one install pipeline: its compile slots bound the node (P8).
    pub installer: Arc<Installer>,
    pub media: Arc<MediaStore>,
    /// The site's URL (`site.url`).
    pub site_url: String,
}

/// The API's shared state (see [`ApiParts`] for the services).
pub struct ApiState {
    pub store: Arc<dyn Store>,
    pub auth: Arc<AuthService>,
    pub users: Arc<UserService>,
    pub content: Arc<ContentService>,
    pub types: Arc<ContentTypeService>,
    pub taxonomy: Arc<TaxonomyService>,
    pub menus: Arc<MenuService>,
    pub options: Arc<OptionService>,
    pub redirects: Arc<RedirectService>,
    pub extensions: Arc<ExtensionAdmin>,
    /// Shared by every request on this node, so [`COMPILE_SLOTS`] bound the node.
    ///
    /// [`COMPILE_SLOTS`]: crate::extensions::COMPILE_SLOTS
    pub installer: Arc<Installer>,
    pub media: Arc<MediaStore>,
    /// `site.url` without a trailing slash.
    pub site_url: String,
    /// `scheme://host[:port]` of `site_url` (lowercase, default port left out): what a
    /// browser sends as `Origin` for the site's own pages (P8).
    pub origin: String,
    /// Whether the session cookie gets `Secure`: `site_url` is `https` (P7).
    pub cookie_secure: bool,
}

/// `site_url` is not an absolute `http`/`https` URL with a host.
#[derive(Debug, thiserror::Error)]
#[error("site URL {0:?} is not an absolute http(s) URL")]
pub struct InvalidSiteUrl(pub String);

impl ApiState {
    pub fn new(parts: ApiParts) -> Result<Self, InvalidSiteUrl> {
        let ApiParts {
            store,
            auth,
            users,
            content,
            types,
            taxonomy,
            menus,
            options,
            redirects,
            extensions,
            installer,
            media,
            site_url,
        } = parts;
        let (origin, cookie_secure) = origin_of(&site_url)?;
        Ok(Self {
            store,
            auth,
            users,
            content,
            types,
            taxonomy,
            menus,
            options,
            redirects,
            extensions,
            installer,
            media,
            site_url: site_url.trim_end_matches('/').to_owned(),
            origin,
            cookie_secure,
        })
    }
}

/// The origin of `url` and whether it is `https`.
pub(crate) fn origin_of(url: &str) -> Result<(String, bool), InvalidSiteUrl> {
    let invalid = || InvalidSiteUrl(url.to_owned());
    let uri: Uri = url.parse().map_err(|_| invalid())?;
    let scheme = uri
        .scheme_str()
        .map(str::to_ascii_lowercase)
        .ok_or_else(invalid)?;
    let default_port = match scheme.as_str() {
        "http" => 80,
        "https" => 443,
        _ => return Err(invalid()),
    };
    let host = uri
        .host()
        .filter(|h| !h.is_empty())
        .ok_or_else(invalid)?
        .to_ascii_lowercase();
    let origin = match uri.port_u16() {
        Some(port) if port != default_port => format!("{scheme}://{host}:{port}"),
        _ => format!("{scheme}://{host}"),
    };
    Ok((origin, scheme == "https"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_from_site_url() {
        let o = |u: &str| origin_of(u).unwrap();
        assert_eq!(
            o("https://example.com"),
            ("https://example.com".into(), true)
        );
        assert_eq!(
            o("https://example.com/"),
            ("https://example.com".into(), true)
        );
        assert_eq!(
            o("http://Example.COM:8080/blog/"),
            ("http://example.com:8080".into(), false)
        );
        assert_eq!(o("http://localhost:80"), ("http://localhost".into(), false));
        assert_eq!(
            o("https://example.com:443/x"),
            ("https://example.com".into(), true)
        );
        assert_eq!(o("http://[::1]:8080"), ("http://[::1]:8080".into(), false));
        for bad in ["example.com", "/path", "ftp://example.com", ""] {
            assert!(origin_of(bad).is_err(), "{bad}");
        }
    }
}
