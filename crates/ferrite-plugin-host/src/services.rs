//! Data the host exposes to plugins, abstracted from storage (design 05 §9).
//!
//! The plugin host never touches the database (or the network) itself: every `content`,
//! `meta`, `options`, `kv`, `settings`, `mail` and `http` import is answered through
//! [`HostServices`]. Capability checks and budgets are applied by the host before these
//! methods are called.
//!
//! **Cancel-safety.** Every method runs inside a plugin call, and the host drops the
//! call's future at the hard deadline; a caller drops it too when its request goes away
//! (a client disconnect) or its job loses the lease. So any await may be the last one:
//! implementations must not hold a `Tx` across an await they cannot finish (a dropped
//! `Tx` rolls back), and each write is a single statement or one short `Tx`, never a
//! sequence of writes that is wrong when cut in the middle.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::manifest::SettingSpec;
use crate::types::Post;

/// Type filter passed to [`HostServices::get_post`]. The explicit `for<'k>` matters:
/// `async_trait` would otherwise tie an elided `&str` to the method's lifetimes.
pub type KindOk = dyn for<'k> Fn(&'k str) -> bool + Sync;

/// Storage-facing services backing the plugin imports.
#[async_trait::async_trait]
pub trait HostServices: Send + Sync + 'static {
    /// Post `id` if its type passes `kind_ok` (the plugin's `content.read` grants).
    /// Implementations should check the type before any expensive loading, so refused
    /// reads cost little; the host re-checks the returned post's type.
    async fn get_post(&self, id: &str, kind_ok: &KindOk) -> Option<Post>;
    /// The type of item `id`, whatever its status (P23): the host lets `meta.get` and
    /// `meta.set` act only on items of the plugin's `content.read` types. Visibility
    /// (which items a plugin may see at all) stays the implementation's, in
    /// [`meta_get`](Self::meta_get) and [`meta_set`](Self::meta_set).
    async fn post_kind(&self, id: &str) -> Option<String>;
    async fn get_post_by_slug(&self, kind: &str, slug: &str) -> Option<Post>;
    async fn list_posts(&self, kind: &str, limit: u32, offset: u32) -> Vec<Post>;
    async fn meta_get(&self, post: &str, key: &str) -> Option<String>;
    async fn meta_set(&self, post: &str, key: &str, value: &str) -> Result<(), String>;
    async fn option_get(&self, key: &str) -> Option<String>;
    async fn kv_get(&self, plugin: &str, key: &str) -> Option<Vec<u8>>;
    async fn kv_set(&self, plugin: &str, key: &str, value: Vec<u8>) -> Result<(), String>;
    async fn kv_delete(&self, plugin: &str, key: &str);
    async fn kv_list(&self, plugin: &str, prefix: &str, limit: u32) -> Vec<String>;
    /// Plugin `plugin`'s setting `key`, declared in its loaded manifest as `spec`, as
    /// JSON text: the stored value if it fits `spec`, else (or when unset) the declared
    /// default or `None`; the host answers the declared default for `None`.
    async fn setting_get(&self, plugin: &str, key: &str, spec: &SettingSpec) -> Option<String>;
    /// Mail from plugin `plugin` to the site's active admins (`mail = "admins"`, P18).
    /// The host has already refused render-path calls, a second mail in one call and
    /// a subject or body outside the limits; implementations add the hourly limit and
    /// the recipients, and must not wait for delivery (F28). The default: mail is not
    /// configured.
    async fn notify_admins(&self, plugin: &str, subject: &str, text: &str) -> Result<(), String> {
        let _ = (plugin, subject, text);
        Err(MAIL_NOT_CONFIGURED.to_owned())
    }
    /// An outbound request of plugin `plugin`, whose manifest lists `hosts` (canonical:
    /// lowercase, no trailing dot). The host has already checked the per-call budget,
    /// the method, `https` on port 443 without userinfo, that `req.url`'s host (made
    /// canonical) is in `hosts`, the request headers and the body size (05 §6 "HTTP
    /// capability details"); implementations resolve, check the addresses and follow
    /// redirects within `hosts`. Errors go to the guest as they are, so they must not
    /// carry internal details (resolved addresses). The default: http is not
    /// configured.
    async fn http_fetch(
        &self,
        plugin: &str,
        hosts: &[String],
        req: HttpRequest,
    ) -> Result<HttpResponse, String> {
        let _ = (plugin, hosts, req);
        Err(HTTP_NOT_CONFIGURED.to_owned())
    }
}

/// What a plugin's `http.fetch` gets when the services have no egress.
pub const HTTP_NOT_CONFIGURED: &str = "http is not configured";

/// An outbound request that passed the host's checks ([`HostServices::http_fetch`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    /// One of `GET HEAD POST PUT PATCH DELETE`.
    pub method: String,
    /// An `https` URL on port 443 without userinfo, its host canonical and listed.
    pub url: url::Url,
    /// Names as the guest gave them (at most 32, none of the refused ones).
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
}

/// The answer to an [`HttpRequest`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// What a plugin's `mail.notify-admins` gets on a site without `[mail]`.
pub const MAIL_NOT_CONFIGURED: &str = "mail is not configured";

/// One mail recorded by [`InMemoryServices`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentMail {
    pub plugin: String,
    pub subject: String,
    pub text: String,
}

#[derive(Default)]
struct Data {
    /// Keyed by post id.
    posts: BTreeMap<String, Post>,
    /// Keyed by (post id, meta key).
    meta: HashMap<(String, String), String>,
    options: HashMap<String, String>,
    /// Keyed by (plugin, key); ordered so prefix listing is deterministic.
    kv: BTreeMap<(String, String), Vec<u8>>,
    /// Keyed by (plugin, key).
    settings: HashMap<(String, String), String>,
    mails: Vec<SentMail>,
    /// Every `http_fetch` call: (plugin, hosts, request).
    fetches: Vec<(String, Vec<String>, HttpRequest)>,
}

/// In-memory [`HostServices`] for tests and the spike.
#[derive(Default)]
pub struct InMemoryServices {
    data: Mutex<Data>,
}

impl InMemoryServices {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert_post(&self, p: Post) {
        self.lock().posts.insert(p.id.clone(), p);
    }

    /// `v` is JSON text, as returned by `options.get`.
    pub fn set_option(&self, k: &str, v: &str) {
        self.lock().options.insert(k.to_string(), v.to_string());
    }

    /// Plugin `plugin`'s setting `key`; `v` is JSON text, as returned by `settings.get`.
    pub fn set_setting(&self, plugin: &str, key: &str, v: &str) {
        self.lock()
            .settings
            .insert(pair(plugin, key), v.to_string());
    }

    /// Every `notify_admins` call so far, in order.
    pub fn mails(&self) -> Vec<SentMail> {
        self.lock().mails.clone()
    }

    /// Every `http_fetch` call so far, in order: (plugin, listed hosts, request).
    pub fn http_fetches(&self) -> Vec<(String, Vec<String>, HttpRequest)> {
        self.lock().fetches.clone()
    }

    fn lock(&self) -> MutexGuard<'_, Data> {
        // The data stays consistent even if a holder panicked: every update is one insert.
        self.data.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn pair(a: &str, b: &str) -> (String, String) {
    (a.to_string(), b.to_string())
}

#[async_trait::async_trait]
impl HostServices for InMemoryServices {
    async fn get_post(&self, id: &str, kind_ok: &KindOk) -> Option<Post> {
        let post = self.lock().posts.get(id).cloned()?;
        kind_ok(&post.kind).then_some(post)
    }

    async fn post_kind(&self, id: &str) -> Option<String> {
        self.lock().posts.get(id).map(|p| p.kind.clone())
    }

    async fn get_post_by_slug(&self, kind: &str, slug: &str) -> Option<Post> {
        self.lock()
            .posts
            .values()
            .find(|p| p.kind == kind && p.slug == slug)
            .cloned()
    }

    async fn list_posts(&self, kind: &str, limit: u32, offset: u32) -> Vec<Post> {
        self.lock()
            .posts
            .values()
            .filter(|p| p.kind == kind)
            .skip(offset as usize)
            .take(limit as usize)
            .cloned()
            .collect()
    }

    async fn meta_get(&self, post: &str, key: &str) -> Option<String> {
        self.lock().meta.get(&pair(post, key)).cloned()
    }

    async fn meta_set(&self, post: &str, key: &str, value: &str) -> Result<(), String> {
        self.lock().meta.insert(pair(post, key), value.to_string());
        Ok(())
    }

    async fn option_get(&self, key: &str) -> Option<String> {
        self.lock().options.get(key).cloned()
    }

    async fn kv_get(&self, plugin: &str, key: &str) -> Option<Vec<u8>> {
        self.lock().kv.get(&pair(plugin, key)).cloned()
    }

    async fn kv_set(&self, plugin: &str, key: &str, value: Vec<u8>) -> Result<(), String> {
        self.lock().kv.insert(pair(plugin, key), value);
        Ok(())
    }

    async fn kv_delete(&self, plugin: &str, key: &str) {
        self.lock().kv.remove(&pair(plugin, key));
    }

    async fn kv_list(&self, plugin: &str, prefix: &str, limit: u32) -> Vec<String> {
        self.lock()
            .kv
            .range(pair(plugin, prefix)..)
            .take_while(|((p, k), _)| p == plugin && k.starts_with(prefix))
            .take(limit as usize)
            .map(|((_, k), _)| k.clone())
            .collect()
    }

    async fn setting_get(&self, plugin: &str, key: &str, _spec: &SettingSpec) -> Option<String> {
        self.lock().settings.get(&pair(plugin, key)).cloned()
    }

    async fn notify_admins(&self, plugin: &str, subject: &str, text: &str) -> Result<(), String> {
        self.lock().mails.push(SentMail {
            plugin: plugin.to_owned(),
            subject: subject.to_owned(),
            text: text.to_owned(),
        });
        Ok(())
    }

    /// Records the request and answers `200` with the body `in-memory`.
    async fn http_fetch(
        &self,
        plugin: &str,
        hosts: &[String],
        req: HttpRequest,
    ) -> Result<HttpResponse, String> {
        self.lock()
            .fetches
            .push((plugin.to_owned(), hosts.to_vec(), req));
        Ok(HttpResponse {
            status: 200,
            headers: Vec::new(),
            body: b"in-memory".to_vec(),
        })
    }
}
