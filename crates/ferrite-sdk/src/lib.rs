//! Guest-side SDK for writing FerriteCMS plugins.
//!
//! A plugin is a `cdylib` crate built for `wasm32-wasip2`. It writes plain functions
//! and wires them to hooks with [`export_plugin!`]; hook priorities live in the
//! plugin's `ferrite-plugin.toml`, not in code.

// The helper modules are used by path (`settings::text`, `viewers::allow`, …); the root
// re-exports only the WIT types and the core handles.
pub mod form;
mod head;
pub mod html;
mod json;
pub mod mail;
pub mod options;
pub mod route;
pub mod settings;
pub mod time;
pub mod viewers;

pub mod bindings {
    //! Raw `wit-bindgen` output for the `ferrite:plugin/plugin@0.1.0` world.
    wit_bindgen::generate!({
        world: "plugin",
        path: "../../wit/ferrite-plugin@0.1.0",
        pub_export_macro: true,
        default_bindings_module: "ferrite_sdk::bindings",
    });
}

use bindings::ferrite::plugin::{
    content, http as wit_http, kv as wit_kv, log as wit_log, settings as wit_settings,
};

pub use bindings::exports::ferrite::plugin::guest::Guest;
pub use bindings::ferrite::plugin::http::{Request as HttpRequest, Response as HttpResponse};
pub use bindings::ferrite::plugin::log::Level;
pub use bindings::ferrite::plugin::types::{
    ActionEvent, FilterValue, LinkTag, MetaTag, PluginError, Post, RequestInfo, RouteRequest,
    RouteResponse, ScriptTag, TermRef,
};
pub use head::Head;

/// Result type used by all hook functions.
pub type Result<T> = core::result::Result<T, PluginError>;

/// Write a line to the host log (always available, no capability needed).
pub fn log(level: Level, msg: &str) {
    wit_log::log(level, msg);
}

/// Per-call context handed to every hook function.
pub struct Ctx {
    req: RequestInfo,
}

impl Ctx {
    #[doc(hidden)]
    pub fn new(req: RequestInfo) -> Self {
        Self { req }
    }

    #[doc(hidden)]
    pub fn empty() -> Self {
        Self::new(RequestInfo {
            path: String::new(),
            locale: String::new(),
            is_logged_in: false,
            post: None,
        })
    }

    /// Request being rendered (path, locale, login state).
    pub fn request(&self) -> &RequestInfo {
        &self.req
    }

    /// The post being rendered, if any. Requires the `content.read` capability.
    pub fn current_post(&self) -> Option<Post> {
        self.req.post.as_deref().and_then(content::get_post)
    }

    /// Plugin-private key/value store. Requires the `kv` capability.
    pub fn kv(&self) -> Kv {
        Kv(())
    }

    /// Outbound HTTPS to manifest-declared hosts. Requires the `http` capability.
    pub fn http(&self) -> Http {
        Http(())
    }

    /// One of this plugin's own settings, as JSON text ([`settings`] decodes the scalar
    /// types).
    pub fn setting(&self, key: &str) -> Option<String> {
        wit_settings::get(key)
    }
}

/// Handle to the `kv` capability.
pub struct Kv(());

impl Kv {
    pub fn get(&self, key: &str) -> Option<Vec<u8>> {
        wit_kv::get(key)
    }

    pub fn set(&self, key: &str, value: &[u8]) -> core::result::Result<(), String> {
        wit_kv::set(key, value)
    }

    pub fn delete(&self, key: &str) {
        wit_kv::delete(key);
    }

    pub fn list_keys(&self, prefix: &str, limit: u32) -> Vec<String> {
        wit_kv::list_keys(prefix, limit)
    }
}

/// Handle to the `http` capability.
pub struct Http(());

impl Http {
    pub fn fetch(&self, req: &HttpRequest) -> core::result::Result<HttpResponse, String> {
        wit_http::fetch(req)
    }
}

#[doc(hidden)]
pub mod __private {
    use super::{Ctx, FilterValue, Head, PluginError, Result};

    /// Re-wraps a filter result in the `filter-value` variant it arrived in.
    pub type Wrap<T> = fn(T) -> FilterValue;

    /// A value a filter function can take and return. Decoding remembers which
    /// `filter-value` variant came in so the same variant goes back out.
    pub trait FilterArg: Sized {
        fn decode(v: FilterValue) -> Result<(Self, Wrap<Self>)>;
    }

    impl FilterArg for Head {
        fn decode(v: FilterValue) -> Result<(Self, Wrap<Self>)> {
            match v {
                FilterValue::Head(h) => Ok((h.into(), |h| FilterValue::Head(h.into()))),
                _ => Err(mismatch("head")),
            }
        }
    }

    impl FilterArg for String {
        fn decode(v: FilterValue) -> Result<(Self, Wrap<Self>)> {
            match v {
                FilterValue::Html(s) => Ok((s, FilterValue::Html)),
                FilterValue::Text(s) => Ok((s, FilterValue::Text)),
                FilterValue::Head(_) => Err(mismatch("html or text")),
            }
        }
    }

    fn mismatch(expected: &str) -> PluginError {
        PluginError::InvalidInput(format!("filter expects a {expected} value"))
    }

    pub fn run_filter<T: FilterArg>(
        f: impl FnOnce(T, &Ctx) -> Result<T>,
        value: FilterValue,
        ctx: &Ctx,
    ) -> Result<FilterValue> {
        let (arg, wrap) = T::decode(value)?;
        f(arg, ctx).map(wrap)
    }

    pub fn unknown_hook(hook: &str) -> PluginError {
        PluginError::InvalidInput(format!("unknown hook {hook}"))
    }
}

/// Export the plugin: maps hook names to functions and generates the `guest` impl.
///
/// ```ignore
/// ferrite_sdk::export_plugin! {
///     filters: { "render.head" => head },
///     actions: { "post.published" => on_publish },
///     routes: { "/ping" => ping },
/// }
/// ```
///
/// * filters: `fn(Head, &Ctx) -> Result<Head>` or `fn(String, &Ctx) -> Result<String>`
/// * actions: `fn(ActionEvent, &Ctx) -> Result<()>`
/// * routes: `fn(RouteRequest, &Ctx) -> Result<RouteResponse>`, matched on exact path
///
/// Unknown hooks yield `invalid-input`; unknown routes yield a 404.
#[macro_export]
macro_rules! export_plugin {
    (
        filters: { $($fh:literal => $ff:path),* $(,)? }
        $(, actions: { $($ah:literal => $af:path),* $(,)? })?
        $(, routes: { $($rp:literal => $rf:path),* $(,)? })?
        $(,)?
    ) => {
        struct __FerritePlugin;

        impl $crate::Guest for __FerritePlugin {
            fn apply_filter(
                hook: ::std::string::String,
                value: $crate::FilterValue,
                req: $crate::RequestInfo,
            ) -> $crate::Result<$crate::FilterValue> {
                let ctx = $crate::Ctx::new(req);
                match hook.as_str() {
                    $($fh => $crate::__private::run_filter($ff, value, &ctx),)*
                    _ => Err($crate::__private::unknown_hook(&hook)),
                }
            }

            fn on_action(
                hook: ::std::string::String,
                event: $crate::ActionEvent,
            ) -> $crate::Result<()> {
                let ctx = $crate::Ctx::empty();
                match hook.as_str() {
                    $($($ah => $af(event, &ctx),)*)?
                    _ => Err($crate::__private::unknown_hook(&hook)),
                }
            }

            fn handle_request(
                req: $crate::RouteRequest,
            ) -> $crate::Result<$crate::RouteResponse> {
                let ctx = $crate::Ctx::empty();
                match req.path.as_str() {
                    $($($rp => $rf(req, &ctx),)*)?
                    _ => Ok($crate::RouteResponse {
                        status: 404,
                        headers: ::std::vec::Vec::new(),
                        body: ::std::vec::Vec::new(),
                    }),
                }
            }
        }

        $crate::bindings::export!(__FerritePlugin);
    };
}
