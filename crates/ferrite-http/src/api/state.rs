//! What every API handler shares: the store and the core services, plus values derived
//! from `site.url` once at boot.

use std::sync::Arc;

use axum::http::Uri;
use ferrite_core::{
    AuthService, ContentService, ContentTypeService, ExtensionAdmin, MediaService, MenuService,
    OptionService, RedirectService, SystemService, TaxonomyService, UserService,
    comments::CommentService, mail::MailQueue,
};
use ferrite_media::MediaStore;
use ferrite_store::Store;

use super::NodeInfo;
use crate::{AppState, extensions::Installer, media::Uploader};

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
    pub media_service: Arc<MediaService>,
    /// The node's one upload pipeline: its slots bound the node (M2a-2b P9).
    pub uploader: Arc<Uploader>,
    /// `media.max_upload`.
    pub max_upload: u64,
    /// Comment submits and moderation (M2a-2b P11–P14).
    pub comments: Arc<CommentService>,
    /// System status, nodes and audit reads (M2a-2b P15).
    pub system: Arc<SystemService>,
    /// The node's public site (its theme, page cache and render counters).
    pub site: Arc<AppState>,
    /// What this node is.
    pub node: NodeInfo,
    /// The node's mail queue (`[mail]` set), shared with the plugin host.
    pub mailer: Option<Arc<MailQueue>>,
    /// The site's URL (`site.url`).
    pub site_url: String,
    /// `site.title` (names the site in the password-reset mail).
    pub site_title: String,
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
    pub media_service: Arc<MediaService>,
    /// Shared by every request on this node, so
    /// [`MEDIA_UPLOAD_SLOTS`](crate::media::MEDIA_UPLOAD_SLOTS) bound the node.
    pub uploader: Arc<Uploader>,
    /// `media.max_upload`: the largest file `POST /media` takes (the body may be 1 MiB
    /// more).
    pub max_upload: u64,
    /// Comment submits and moderation (M2a-2b P11–P14).
    pub comments: Arc<CommentService>,
    /// System status, nodes and audit reads (M2a-2b P15).
    pub system: Arc<SystemService>,
    /// The node's public site (its theme, page cache and render counters).
    pub site: Arc<AppState>,
    /// What this node is (`GET /system/status`).
    pub node: NodeInfo,
    /// The node's mail queue: sends run in the background (`None`: no `[mail]`, the
    /// password reset is `503 mail-unavailable`).
    pub mailer: Option<Arc<MailQueue>>,
    /// `site.url` without a trailing slash.
    pub site_url: String,
    /// `site.title`.
    pub site_title: String,
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
            media_service,
            uploader,
            max_upload,
            comments,
            system,
            site,
            node,
            mailer,
            site_url,
            site_title,
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
            media_service,
            uploader,
            max_upload,
            comments,
            system,
            site,
            node,
            mailer,
            site_url: site_url.trim_end_matches('/').to_owned(),
            site_title,
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
