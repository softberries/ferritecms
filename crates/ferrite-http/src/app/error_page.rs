//! Error pages (06 §7): the theme's template when it has one, else a built-in page.
//! Never cached (status >= 400).

use std::sync::Arc;

use ferrite_cache::CachedPage;
use ferrite_theme::{
    HelperBridge, NoBridge, RenderContext, RequestInfo, TemplateKey, error_template,
    resolve_template,
};
use tokio::runtime::Handle;

use super::{
    AppState,
    bridge::BlockingBridge,
    pipeline::{html_page, render_blocking},
};
use crate::head::{HeadPage, HeadSite, core_head, render_head};

/// Which error page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ErrorKind {
    /// 404: the theme's hierarchy for not-found (`404.html`, else `index.html`), with
    /// the data helpers (menus) available.
    NotFound,
    /// 410 (a `410` redirect row): rendered like the 404 page, `request.status` 410.
    Gone,
    /// 500: the theme's `500.html` with minimal context and no data helpers, else the
    /// built-in page.
    Server,
}

impl ErrorKind {
    fn status(self) -> u16 {
        match self {
            Self::NotFound => 404,
            Self::Gone => 410,
            Self::Server => 500,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::NotFound => "Page not found",
            Self::Gone => "Page gone",
            Self::Server => "Something went wrong",
        }
    }
}

/// The error page for `kind` at `path`. A failing theme render is logged (with the
/// template's file and line) and answered with the built-in page.
pub(crate) async fn render(state: &Arc<AppState>, path: &str, kind: ErrorKind) -> CachedPage {
    let status = kind.status();
    let theme = state.theme.load_full();
    let template = match kind {
        ErrorKind::NotFound | ErrorKind::Gone => {
            Some(resolve_template(&TemplateKey::NotFound, |n| {
                theme.has_template(n)
            }))
        }
        ErrorKind::Server => error_template(|n| theme.has_template(n)).map(str::to_owned),
    };
    let Some(template) = template else {
        return builtin(kind, &state.config.site_title);
    };
    // A 500 may come from the store itself: minimal context, config defaults only.
    let site = match kind {
        ErrorKind::Server => state.fallback_site(&theme),
        ErrorKind::NotFound | ErrorKind::Gone => {
            let mut ignored = Default::default();
            match state.site_info(&theme, &mut ignored).await {
                Ok(site) => site,
                Err(e) => {
                    tracing::warn!(error = %e, "site options unreadable for the error page");
                    state.fallback_site(&theme)
                }
            }
        }
    };
    let head_site = HeadSite {
        title: &site.title,
        url: &site.url,
        description: &site.description,
    };
    let mut head = core_head(head_site, HeadPage { item: None, path }, None);
    head.title = Some(format!("{} – {}", kind.title(), site.title));
    let head_html = render_head(&head, head_site);
    let bridge: Arc<dyn HelperBridge> = match kind {
        ErrorKind::Server => Arc::new(NoBridge),
        ErrorKind::NotFound | ErrorKind::Gone => Arc::new(BlockingBridge::new(
            Handle::current(),
            state.reader.clone(),
            state.store.clone(),
        )),
    };
    let title = site.title.clone();
    let ctx = RenderContext {
        site,
        request: RequestInfo {
            path: path.to_owned(),
            query_s: None,
            page: 1,
        },
        head_html,
        status,
        ..RenderContext::default()
    };
    match render_blocking(state, theme, template.clone(), ctx, bridge).await {
        Ok(out) => html_page(status, out.html, Vec::new(), None),
        Err(e) => {
            tracing::error!(%template, status, error = %e, "error page render failed; built-in page used");
            builtin(kind, &title)
        }
    }
}

/// The built-in error page: plain HTML, no theme involved.
pub(crate) fn builtin(kind: ErrorKind, site_title: &str) -> CachedPage {
    let title = html_escape::encode_text(kind.title());
    let site = html_escape::encode_text(site_title);
    let message = match kind {
        ErrorKind::NotFound => "The page you asked for does not exist.",
        ErrorKind::Gone => "The page you asked for has been removed.",
        ErrorKind::Server => "The page could not be shown. Please try again later.",
    };
    let html = format!(
        "<!doctype html>\n<html lang=\"en\">\n<head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>{title} – {site}</title></head>\n<body><main><h1>{title}</h1>\
         <p>{message}</p><p><a href=\"/\">Home</a></p></main></body>\n</html>\n"
    );
    html_page(kind.status(), html, Vec::new(), None)
}
