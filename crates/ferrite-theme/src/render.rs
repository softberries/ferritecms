//! The sandboxed renderer (06 §4).
//!
//! # Sandbox
//!
//! * Auto-escape (ruling R15): HTML escaping for every template except names ending
//!   in `.txt`, `.css` or `.js`, so a new kind of markup template is safe by default.
//! * Undefined values are chainable (`post.author.login` with no `post` is undefined
//!   and prints as nothing), so templates need fewer guards.
//! * Templates come only from the theme's `templates/` folder, compiled into the
//!   theme's environment at load. There is **no loader**: `include`/`extends`/`import`
//!   resolve names against those compiled templates only, verbatim (no path joining),
//!   so `../../etc/passwd` or `/abs` are just unknown template names.
//! * Fuel: [`FUEL`] instructions per render. MiniJinja creates a fresh fuel tracker
//!   for every render's `State`, so the environment-wide setting is per render.
//! * Output: at most [`MAX_OUTPUT`] bytes, enforced by a capped `io::Write` sink, so
//!   rendering stops at the first write over the cap. Recursion depth uses MiniJinja's
//!   default limit (500; includes and macros cost more than one level).
//! * Exhausted fuel, an output over the cap, the recursion limit, and MiniJinja's own
//!   size guards (`range()` over 100 000 items, string/sequence repetition over 100 M)
//!   are [`ThemeError::Limit`]. Each render has its own state, so the next one is
//!   unaffected.
//! * Data-backed helpers (`related`, `query`, `menu`, `image`, `comments`, `page_tree`)
//!   go through the [`HelperBridge`], at most [`MAX_BRIDGE_CALLS`] times per render;
//!   after that they return nothing and one warning is logged. Same-name `menu()` calls
//!   and same-kind `page_tree()` calls are memoised, and `image()` of media already in
//!   the context (featured images, and the featured images of a `page_tree()` the
//!   template read, M4-4) needs no call.
//!
//! **Known gaps (ruling R14).** Fuel counts instructions, not bytes or CPU time:
//!
//! * *Memory:* a few instructions can build large intermediate strings (`~`
//!   concatenation doubling in a loop) before anything reaches the output cap;
//!   MiniJinja has no allocation hook.
//! * *CPU per instruction:* one instruction can do a lot of native work (`sort`,
//!   `join`, `replace` or `unique` over a 100 000-item sequence, or one repetition
//!   just under the 100 M guard), so wall time is bounded only loosely by fuel.
//!
//! Themes are installed by administrators, so these are robustness rather than
//! privilege issues.
//!
//! # Globals
//!
//! Templates see `site`, `post`, `page` (set for items of kind `page`), `posts`,
//! `archive` (listing routes: `kind`, `term`, `author`, `year`, `month`; ruling R30),
//! `pagination` (`page`, `pages`, `prev_url`, `next_url`), `request` (`path`,
//! `s` = the search query, `page`, `status`, `preview`) and the
//! helpers `menu(name)`, `related(post, n=3)`,
//! `query(type=…, term=…, limit=10, order="date"|"menu")`, `page_tree(post=none,
//! kind=none)` (the public items of one type as a tree: see the `page_tree` module),
//! `asset(path)`, `image(media, size)`, `head()`, `footer()` (the plugins'
//! `render.footer` HTML, safe; empty without footer plugins), `plugin_route(name, path)` (a plugin
//! route's URL, `/_p/<name><path>`), `comments(post)` (page `?cpage=N` of the public
//! thread of a post, or none: `open`, `count` = `total` (every approved comment),
//! `page`, `pages`, `per_page`, `prev_url`, `next_url` (the neighbouring comment pages
//! or none), `items`, `action` = `/_comment`, `honeypot` = `website`; each
//! item `id`, `parent_id`, `depth`, `author_name`, `author_url`, `html` (core-rendered,
//! safe), `date`, `can_reply`). Custom filters: `date(fmt)`,
//! `excerpt(n)`, `slugify`, `json`.
//!
//! MiniJinja builtins that remain: the globals `range` (≤ 100 000 items), `dict`,
//! `namespace`; the standard filters (`escape`, `safe`, `default`, `length`, `join`,
//! `sort`, `map`, `tojson`, …) and tests. None of them reach the filesystem, network,
//! environment or clock. `debug()` is removed (it dumps the whole render state), and
//! the crate is built without the `debug` feature, so errors carry name and line but
//! not source snippets. `urlencode` (MiniJinja's) percent-encodes everything except
//! ASCII alphanumerics, `/`, `.`, `-` and `_`; its output is a plain string, so
//! auto-escaping still applies, and since `:` is encoded it cannot form a URL scheme.
//!
//! # Dependency keys
//!
//! A render records `theme`, `option:site.title`, `option:site.description`,
//! `option:site.locale` (`site.url` is config, not an option, so it has no key); for every item in the context or
//! returned by a helper `post:<id>`, `user:<author>`, `term:<id>` per term and
//! `post:<featured media>`; `term:<id>`/`user:<id>` for `archive.term`/`archive.author`;
//! `archive:<kind>` for `related`/`query`/`page_tree`; `menu:<name>` for
//! menus; `post:<media>` for `image()`; `post:<id>` for `comments()`; plus every key the
//! [`HelperBridge`] reports for `related`, `query`, `menu` (menu targets, hidden ones
//! included) and `comments` (`option:comments.enabled`, `option:comments.per_page`,
//! the permalink's keys). The keys
//! of the `SiteRead` that produced the context (ancestors, listings, options) are the
//! caller's to merge, as are `plugins` and the theme-settings option.

use std::{
    collections::{BTreeMap, HashMap},
    io,
    sync::{Arc, Mutex, MutexGuard},
};

use ferrite_core::{
    DepKeys, Listing, MediaPublic, PageItem, TermPublic, TranslationLink, UserPublic,
    comments::{COMMENT_FORM_PATH, CommentThread, HONEYPOT_FIELD},
    events::{THEME_KEY, archive_key, menu_key, option_key, post_key, term_key, user_key},
    is_type_name_shaped,
    site::{ContentTree, ListOrder},
};
use ferrite_domain::ContentId;
use minijinja::{
    AutoEscape, Environment, Error, ErrorKind, UndefinedBehavior,
    value::{Kwargs, Serde, Value},
};
use serde::Serialize;

use crate::{
    HelperBridge, ImageRef, Theme, ThemeError,
    bridge::{MAX_QUERY_LIMIT, MAX_RELATED},
    filters,
    package::{SEGMENT, asset_url},
    page_tree::tree_value,
    view::ItemObject,
};

/// Instructions one render may execute.
pub const FUEL: u64 = 5_000_000;
/// Most bytes one render may output (8 MiB).
pub const MAX_OUTPUT: usize = 8 * 1024 * 1024;
/// Most [`HelperBridge`] calls one render may make.
pub const MAX_BRIDGE_CALLS: u32 = 20;
const DEFAULT_RELATED: u32 = 3;
/// Most `page_tree()` values (one per kind and current node) one render keeps.
const MAX_TREE_VALUES: usize = 16;
const DEFAULT_QUERY_LIMIT: u32 = 10;
/// Options every render reads through `site`. `site.url` is not one: it comes from the
/// config, which no write changes at runtime.
const SITE_OPTIONS: [&str; 3] = ["site.title", "site.description", "site.locale"];
/// MiniJinja error details that mean "a limit was hit" rather than "the template is
/// wrong". Pinned by tests, since they are MiniJinja's strings.
const LIMIT_DETAILS: [&str; 4] = [
    "recursion limit exceeded",
    "range has too many elements",
    "repeated string is too large",
    "repeated sequence is too large",
];

/// `site` in templates: whitelisted options plus the active theme's settings.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct SiteInfo {
    pub title: String,
    pub url: String,
    pub description: String,
    pub locale: String,
    /// IANA zone the `date` filter formats in (`site.timezone` of the config, validated
    /// at load); empty means UTC.
    pub timezone: String,
    /// Every declared setting with its value
    /// ([`ThemeManifest::settings_values`](crate::ThemeManifest::settings_values)).
    pub settings: BTreeMap<String, serde_json::Value>,
    /// The site's locales, the default first (13 §5): one entry on a one-locale site.
    pub locales: Vec<LocaleLink>,
    /// The default locale's tag.
    pub default_locale: String,
}

/// One entry of `site.locales` in templates: the locale and its root.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct LocaleLink {
    pub tag: String,
    pub label: String,
    /// The locale's root path (`/`, `/de/`).
    pub url: String,
    /// The request's locale.
    pub current: bool,
}

/// `request` in templates.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequestInfo {
    pub path: String,
    /// The search query (`?s=`); `request.s` in templates.
    pub query_s: Option<String>,
    /// 1-based listing page.
    pub page: u32,
    /// 1-based comment page of a single item (`?cpage=N`, P34); what `comments()`
    /// shows. 0 is page 1.
    pub cpage: u32,
}

/// `archive` in templates: what a listing route lists (ruling R30). Set by the caller
/// for every listing route (front posts, type, term, author, date, search); `None` for
/// singles and error pages.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ArchiveInfo {
    /// `front`, `type`, `term`, `author`, `date` or `search`.
    pub kind: String,
    /// Content type of a type archive (`post`, `event`, ...).
    pub content_type: Option<String>,
    pub term: Option<TermPublic>,
    pub author: Option<UserPublic>,
    pub year: Option<i32>,
    /// 1..=12.
    pub month: Option<u32>,
}

/// `pagination` in templates: the listing's position and ready-made, already encoded
/// links to the neighbouring pages (`None` at either end). Set by the caller, which
/// knows the route's URL scheme.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Pagination {
    pub page: u32,
    pub pages: u32,
    pub prev_url: Option<String>,
    pub next_url: Option<String>,
}

/// Everything a template may see; built from preloaded data (plan ruling P2).
#[derive(Debug, Clone, Default)]
pub struct RenderContext {
    pub site: SiteInfo,
    /// The single item, if any (`post`, and `page` for kind `page`).
    pub item: Option<PageItem>,
    /// The listing, if any (`posts`).
    pub listing: Option<Listing>,
    /// What the listing lists (`archive`).
    pub archive: Option<ArchiveInfo>,
    /// Links to neighbouring listing pages (`pagination`).
    pub pagination: Option<Pagination>,
    pub request: RequestInfo,
    /// Pre-rendered `<head>` HTML (core SEO + plugin filters); `head()` returns it
    /// unescaped, so it must already be safe.
    pub head_html: String,
    /// Plugin footer HTML (the `render.footer` filter, sanitised by the plugin host
    /// unless a plugin is `render.html = "raw"`); `footer()` returns it unescaped, so it
    /// must already be safe. Empty without footer plugins and on error pages.
    pub footer_html: String,
    pub status: u16,
    pub preview: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RenderOutput {
    pub html: String,
    pub deps: DepKeys,
}

/// Renders theme templates on the calling (blocking) thread.
#[derive(Debug, Clone)]
pub struct Renderer {
    max_output: usize,
    max_bridge_calls: u32,
}

impl Default for Renderer {
    fn default() -> Self {
        Self {
            max_output: MAX_OUTPUT,
            max_bridge_calls: MAX_BRIDGE_CALLS,
        }
    }
}

impl Renderer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Renders `templates/<template>` of `theme` with `ctx`. See the module docs for
    /// limits, globals and the dependency keys returned.
    pub fn render(
        &self,
        theme: &Theme,
        template: &str,
        ctx: RenderContext,
        bridge: Arc<dyn HelperBridge>,
    ) -> Result<RenderOutput, ThemeError> {
        if !theme.has_template(template) {
            return Err(ThemeError::TemplateNotFound(template.to_owned()));
        }
        let tmpl = theme
            .env
            .get_template(template)
            .map_err(|_| ThemeError::TemplateNotFound(template.to_owned()))?;
        let scope = Arc::new(Scope {
            bridge,
            hash: theme.hash().to_owned(),
            cpage: ctx.request.cpage.max(1),
            max_calls: self.max_bridge_calls,
            locales: ctx.site.locales.clone(),
            strings: theme.manifest().strings_for(&ctx.site.locale),
            state: Mutex::new(ScopeState::default()),
        });
        {
            let mut st = scope.lock();
            st.deps.insert(THEME_KEY.to_owned());
            for key in SITE_OPTIONS {
                st.deps.insert(option_key(key));
            }
        }
        let root = root_value(ctx, &scope);
        let mut sink = CappedSink {
            buf: Vec::new(),
            cap: self.max_output,
            exceeded: false,
        };
        let result = tmpl.render_captured_to(root, &mut sink).map(drop);
        if let Err(e) = result {
            return Err(classify(&e, sink.exceeded, self.max_output));
        }
        let html = String::from_utf8(sink.buf)
            .map_err(|_| ThemeError::Render("output is not UTF-8".into()))?;
        let deps = scope.lock().deps.clone();
        Ok(RenderOutput { html, deps })
    }
}

/// The theme's environment: every template compiled, sandbox settings applied.
pub(crate) fn build_env(
    sources: BTreeMap<String, String>,
) -> Result<Environment<'static>, Vec<String>> {
    let mut env = Environment::new();
    env.remove_global("debug");
    // `post.title` on a listing page (no `post`) is undefined, not an error.
    env.set_undefined_behavior(UndefinedBehavior::Chainable);
    env.set_fuel(Some(FUEL));
    env.set_auto_escape_callback(|name| {
        if [".txt", ".css", ".js"]
            .iter()
            .any(|ext| name.ends_with(ext))
        {
            AutoEscape::None
        } else {
            AutoEscape::Html
        }
    });
    env.add_filter("date", filters::date);
    env.add_filter("excerpt", filters::excerpt);
    env.add_filter("slugify", filters::slugify);
    env.add_filter("json", filters::json);
    env.add_function("plugin_route", plugin_route);
    let mut errors = Vec::new();
    for (name, src) in sources {
        if let Err(e) = env.add_template_owned(name.clone(), src) {
            errors.push(describe(&e, &name));
        }
    }
    if errors.is_empty() {
        Ok(env)
    } else {
        Err(errors)
    }
}

/// `plugin_route(name, path)`: the URL of a plugin's route, `/_p/<name><path>` (05
/// §4b), each path segment percent-encoded. `name` must be a plugin name
/// (`[a-z0-9-]{2,48}`) and `path` start with `/` and hold no `.`/`..` segment; anything
/// else is `""` (a template typo yields an empty link, not a URL elsewhere).
pub(crate) fn plugin_route(name: &str, path: &str) -> String {
    let name_ok = ferrite_domain::valid_plugin_name(name);
    let Some(rest) = path.strip_prefix('/') else {
        return String::new();
    };
    if !name_ok || rest.split('/').any(|seg| seg == "." || seg == "..") {
        return String::new();
    }
    let mut url = format!("/_p/{name}");
    for seg in rest.split('/') {
        url.push('/');
        url.extend(percent_encoding::utf8_percent_encode(seg, SEGMENT));
    }
    url
}

/// The MiniJinja errors in `err`'s source chain, outermost first (errors inside an
/// include are wrapped in a `BadInclude`).
fn chain(err: &Error) -> impl Iterator<Item = &Error> {
    let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(err);
    std::iter::from_fn(move || {
        let e = cur?;
        cur = e.source();
        Some(e)
    })
    .filter_map(|e| e.downcast_ref::<Error>())
}

/// `templates/<file>:<line>: <message>` for the innermost error that has a location.
fn describe(err: &Error, fallback_name: &str) -> String {
    let inner = chain(err)
        .filter(|e| e.name().is_some())
        .last()
        .unwrap_or(err);
    let name = inner.name().unwrap_or(fallback_name);
    let line = inner.line().unwrap_or(0);
    let msg = match inner.detail() {
        Some(d) => format!("{}: {d}", inner.kind()),
        None => inner.kind().to_string(),
    };
    format!("templates/{name}:{line}: {msg}")
}

fn classify(err: &Error, overflow: bool, cap: usize) -> ThemeError {
    if overflow {
        return ThemeError::Limit(format!("output exceeds {cap} bytes"));
    }
    for e in chain(err) {
        if e.kind() == ErrorKind::OutOfFuel {
            return ThemeError::Limit(format!(
                "fuel exhausted ({FUEL} instructions): {}",
                describe(err, "?")
            ));
        }
        if e.kind() == ErrorKind::InvalidOperation
            && e.detail().is_some_and(|d| LIMIT_DETAILS.contains(&d))
        {
            return ThemeError::Limit(describe(err, "?"));
        }
    }
    ThemeError::Render(describe(err, "?"))
}

/// Rejects (without writing) any write that would take the output past `cap`.
struct CappedSink {
    buf: Vec<u8>,
    cap: usize,
    exceeded: bool,
}

impl io::Write for CappedSink {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if self.buf.len().saturating_add(data.len()) > self.cap {
            self.exceeded = true;
            return Err(io::Error::other("output limit exceeded"));
        }
        self.buf.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Per-render state shared by the helper closures.
struct Scope {
    bridge: Arc<dyn HelperBridge>,
    hash: String,
    /// The comment page `comments()` asks for ([`RequestInfo::cpage`]).
    cpage: u32,
    max_calls: u32,
    /// `site.locales`: the switcher's entries.
    locales: Vec<LocaleLink>,
    /// The theme's `[strings]` for the render's locale (its language's, then its own).
    strings: HashMap<String, String>,
    state: Mutex<ScopeState>,
}

#[derive(Default)]
struct ScopeState {
    deps: DepKeys,
    calls: u32,
    warned: bool,
    menus: HashMap<String, Value>,
    /// `page_tree()` reads by kind (memo key `tree:<kind>`, P57).
    trees: HashMap<String, Option<Arc<ContentTree>>>,
    /// `page_tree()` values by kind and current node, so a template calling it in a loop
    /// builds each one once; at most [`MAX_TREE_VALUES`] entries, later ones are built
    /// on every call.
    tree_values: HashMap<(String, Option<ContentId>), Value>,
    /// Media already in the context (featured images of items and of tree nodes), for
    /// `image()` without a bridge call.
    media: HashMap<ContentId, MediaPublic>,
    /// `translations()` results by item (M6-1), one bridge call per item.
    translations: HashMap<ContentId, Vec<TranslationLink>>,
}

impl Scope {
    fn lock(&self) -> MutexGuard<'_, ScopeState> {
        // A panic while holding the lock cannot leave the state inconsistent in a way
        // that matters (it only accumulates keys), so a poisoned lock is still used.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn record(&self, key: String) {
        self.lock().deps.insert(key);
    }

    /// Takes one bridge call from the budget; `false` (and one warning) when spent.
    fn take_call(&self, helper: &str) -> bool {
        let mut st = self.lock();
        if st.calls >= self.max_calls {
            if !st.warned {
                st.warned = true;
                tracing::warn!(
                    helper,
                    max = self.max_calls,
                    "theme helper call budget spent; further data helpers return nothing"
                );
            }
            return false;
        }
        st.calls += 1;
        true
    }

    /// Records `item`'s keys and wraps it for templates.
    fn item(&self, item: PageItem) -> Value {
        let mut st = self.lock();
        st.deps.insert(post_key(item.item.id));
        if let Some(a) = &item.author {
            st.deps.insert(user_key(a.id));
        }
        for t in &item.terms {
            st.deps.insert(term_key(t.id));
        }
        if let Some(m) = &item.featured {
            st.deps.insert(post_key(m.id));
            st.media.insert(m.id, m.clone());
        }
        drop(st);
        Value::from_object(ItemObject(item))
    }

    fn items(&self, items: Vec<PageItem>) -> Value {
        Value::from(items.into_iter().map(|i| self.item(i)).collect::<Vec<_>>())
    }

    /// Merges keys a bridge call reported.
    fn merge(&self, deps: DepKeys) {
        self.lock().deps.extend(deps);
    }
}

fn root_value(ctx: RenderContext, scope: &Arc<Scope>) -> Value {
    let mut root: BTreeMap<&'static str, Value> = BTreeMap::new();
    root.insert("site", Value::from(Serde(&ctx.site)));
    if let Some(item) = ctx.item {
        let is_page = item.item.kind == "page";
        let v = scope.item(item);
        if is_page {
            root.insert("page", v.clone());
        }
        root.insert("post", v);
    }
    if let Some(l) = ctx.listing {
        root.insert("posts", listing_value(l, scope));
    }
    if let Some(a) = ctx.archive {
        if let Some(t) = &a.term {
            scope.record(term_key(t.id));
        }
        if let Some(u) = &a.author {
            scope.record(user_key(u.id));
        }
        root.insert("archive", Value::from(Serde(&a)));
    }
    if let Some(p) = ctx.pagination {
        root.insert("pagination", Value::from(Serde(&p)));
    }
    root.insert(
        "request",
        minijinja::context! {
            path => ctx.request.path,
            s => ctx.request.query_s,
            page => ctx.request.page.max(1),
            status => ctx.status,
            preview => ctx.preview,
        },
    );
    let head = ctx.head_html;
    root.insert(
        "head",
        Value::from_function(move || Value::from_safe_string(head.clone())),
    );
    let footer = ctx.footer_html;
    root.insert(
        "footer",
        Value::from_function(move || Value::from_safe_string(footer.clone())),
    );
    let s = scope.clone();
    root.insert(
        "asset",
        Value::from_function(move |path: &str| asset_url(&s.hash, path)),
    );
    let s = scope.clone();
    root.insert(
        "menu",
        Value::from_function(move |name: &str| menu(&s, name)),
    );
    let s = scope.clone();
    root.insert(
        "related",
        Value::from_function(move |item: &Value, n: Option<u32>| related(&s, item, n)),
    );
    let s = scope.clone();
    root.insert(
        "query",
        Value::from_function(move |kwargs: Kwargs| query(&s, kwargs)),
    );
    let s = scope.clone();
    root.insert(
        "page_tree",
        Value::from_function(move |post: Option<Value>, kwargs: Kwargs| {
            page_tree(&s, post, kwargs)
        }),
    );
    let s = scope.clone();
    root.insert(
        "image",
        Value::from_function(move |media: &Value, size: Option<&str>| image(&s, media, size)),
    );
    let s = scope.clone();
    root.insert(
        "comments",
        Value::from_function(move |item: &Value| comments(&s, item)),
    );
    let s = scope.clone();
    root.insert(
        "translations",
        Value::from_function(move |item: &Value| Value::from(Serde(translations(&s, item)))),
    );
    let s = scope.clone();
    root.insert(
        "locale_links",
        Value::from_function(move |post: Option<Value>| locale_links(&s, post)),
    );
    let s = scope.clone();
    root.insert(
        "t",
        Value::from_function(move |text: &str| {
            s.strings.get(text).map_or(text, String::as_str).to_owned()
        }),
    );
    Value::from(root)
}

/// The public translations of `item` (`SiteReader::translations`, 13 §5): one bridge
/// call per item and render (memoised), none on a one-locale site.
fn translations(scope: &Scope, item: &Value) -> Vec<TranslationLink> {
    if scope.locales.len() < 2 {
        return Vec::new();
    }
    let Some(id) = content_id(item) else {
        return Vec::new();
    };
    if let Some(found) = scope.lock().translations.get(&id) {
        return found.clone();
    }
    if !scope.take_call("translations") {
        return Vec::new();
    }
    let (links, deps) = scope.bridge.translations(id);
    scope.merge(deps);
    scope.lock().translations.insert(id, links.clone());
    links
}

/// One entry per site locale for a language switcher (13 §5): `{ tag, label, url,
/// current, translated }`, the url being `post`'s translation in that locale when it
/// exists, else the locale's root.
fn locale_links(scope: &Scope, post: Option<Value>) -> Value {
    let found = post
        .filter(|p| !p.is_undefined() && !p.is_none())
        .map(|p| translations(scope, &p))
        .unwrap_or_default();
    let links: Vec<Value> = scope
        .locales
        .iter()
        .map(|l| {
            let t = found.iter().find(|t| t.locale == l.tag);
            minijinja::context! {
                tag => l.tag.clone(),
                label => l.label.clone(),
                url => t.map_or(l.url.as_str(), |t| t.url.as_str()),
                current => l.current,
                translated => t.is_some(),
            }
        })
        .collect();
    Value::from(links)
}

fn listing_value(l: Listing, scope: &Scope) -> Value {
    let per_page = u64::from(l.per_page.max(1));
    let pages = l.total.div_ceil(per_page).max(1);
    let page = u64::from(l.page.max(1));
    minijinja::context! {
        items => scope.items(l.items),
        page => page,
        per_page => per_page,
        total => l.total,
        pages => pages,
        prev_page => (page > 1).then(|| page - 1),
        next_page => (page < pages).then(|| page + 1),
    }
}

/// A content id from an item object, a map with `id` (e.g. `post.featured`) or a
/// UUID string.
fn content_id(v: &Value) -> Option<ContentId> {
    if let Some(obj) = v.downcast_object_ref::<ItemObject>() {
        return Some(obj.0.item.id);
    }
    if let Some(s) = v.as_str() {
        return s.parse().ok();
    }
    v.get_attr("id").ok()?.as_str()?.parse().ok()
}

fn menu(scope: &Scope, name: &str) -> Value {
    scope.record(menu_key(name));
    if let Some(v) = scope.lock().menus.get(name) {
        return v.clone();
    }
    if !scope.take_call("menu") {
        return Value::from(());
    }
    let (tree, deps) = scope.bridge.menu(name);
    scope.merge(deps);
    let v = tree.map_or(Value::from(()), |t| Value::from(Serde(&t)));
    scope.lock().menus.insert(name.to_owned(), v.clone());
    v
}

fn related(scope: &Scope, item: &Value, n: Option<u32>) -> Value {
    let Some(id) = content_id(item) else {
        return Value::from(Vec::<Value>::new());
    };
    if let Some(obj) = item.downcast_object_ref::<ItemObject>() {
        scope.record(archive_key(&obj.0.item.kind));
    }
    if !scope.take_call("related") {
        return Value::from(Vec::<Value>::new());
    }
    let n = n.unwrap_or(DEFAULT_RELATED).clamp(1, MAX_RELATED);
    let (items, deps) = scope.bridge.related(id, n);
    scope.merge(deps);
    for i in &items {
        scope.record(archive_key(&i.item.kind));
    }
    scope.items(items)
}

fn query(scope: &Scope, kwargs: Kwargs) -> Result<Value, Error> {
    let kind: String = kwargs.get("type")?;
    let term: Option<String> = kwargs.get("term")?;
    let limit: Option<u32> = kwargs.get("limit")?;
    let order: Option<String> = kwargs.get("order")?;
    kwargs.assert_all_used()?;
    let order = match order.as_deref() {
        None | Some("date") => ListOrder::Date,
        Some("menu") => ListOrder::Menu,
        Some(other) => {
            return Err(Error::new(
                ErrorKind::InvalidOperation,
                format!("query(): order must be \"date\" or \"menu\", not {other:?}"),
            ));
        }
    };
    scope.record(archive_key(&kind));
    if !scope.take_call("query") {
        return Ok(Value::from(Vec::<Value>::new()));
    }
    let limit = limit
        .unwrap_or(DEFAULT_QUERY_LIMIT)
        .clamp(1, MAX_QUERY_LIMIT);
    let (items, deps) = scope.bridge.query(&kind, term.as_deref(), limit, order);
    scope.merge(deps);
    Ok(scope.items(items))
}

/// A value that is absent for a helper argument: missing, `none` or undefined.
fn given(v: Option<Value>) -> Option<Value> {
    v.filter(|v| !v.is_none() && !v.is_undefined())
}

/// `page_tree(post=none, kind=none)` (P57): the tree of `kind`, else of `post.kind`,
/// with `post` marked current (see the `page_tree` module for the shape). Without a
/// kind, or with one that is not a type name, it is `none` and makes no call; a type
/// without public items, or a spent call budget, also gives `none`. One bridge call per
/// kind and render. Records `archive:<kind>`.
fn page_tree(scope: &Scope, post: Option<Value>, kwargs: Kwargs) -> Result<Value, Error> {
    let named: Option<Value> = kwargs.get("post")?;
    let kind: Option<Value> = kwargs.get("kind")?;
    kwargs.assert_all_used()?;
    let post = given(post).or(given(named));
    let kind = match given(kind) {
        Some(k) => k.as_str().map(str::to_owned),
        None => post.as_ref().and_then(item_kind),
    };
    let Some(kind) = kind.filter(|k| is_type_name_shaped(k)) else {
        return Ok(Value::from(()));
    };
    scope.record(archive_key(&kind));
    let memo = scope.lock().trees.get(&kind).cloned();
    let tree = match memo {
        Some(tree) => tree,
        None => {
            if !scope.take_call("page_tree") {
                return Ok(Value::from(()));
            }
            let (tree, deps) = scope.bridge.tree(&kind);
            scope.merge(deps);
            let tree = tree.map(Arc::new);
            let mut st = scope.lock();
            // The media the tree checked: `image()` on a node's `featured` needs no
            // bridge call (its `post:<media>` key is in the tree's deps already).
            if let Some(t) = &tree {
                for (id, m) in &t.media {
                    st.media.entry(*id).or_insert_with(|| m.clone());
                }
            }
            st.trees.insert(kind.clone(), tree.clone());
            drop(st);
            tree
        }
    };
    let Some(tree) = tree else {
        return Ok(Value::from(()));
    };
    let key = (kind, post.as_ref().and_then(content_id));
    if let Some(v) = scope.lock().tree_values.get(&key) {
        return Ok(v.clone());
    }
    let v = tree_value(&tree, key.1);
    let mut st = scope.lock();
    if st.tree_values.len() < MAX_TREE_VALUES {
        st.tree_values.insert(key, v.clone());
    }
    Ok(v)
}

/// The content type of an item object, or of a map with `kind`.
fn item_kind(v: &Value) -> Option<String> {
    if let Some(obj) = v.downcast_object_ref::<ItemObject>() {
        return Some(obj.0.item.kind.clone());
    }
    v.get_attr("kind").ok()?.as_str().map(str::to_owned)
}

fn image(scope: &Scope, media: &Value, size: Option<&str>) -> Value {
    let Some(id) = content_id(media) else {
        return Value::from(());
    };
    scope.record(post_key(id));
    let size = size.unwrap_or("full");
    let local = scope.lock().media.get(&id).cloned();
    let found = match local {
        Some(m) => Some(ImageRef::from_media(&m, size)),
        None if scope.take_call("image") => scope.bridge.image(id, size),
        None => None,
    };
    found.map_or(Value::from(()), |r| Value::from(Serde(&r)))
}

/// `comments(post)`: the request's page (`request.cpage`) of the post's public thread,
/// or none (not a public post, an unknown id, the call budget spent). Records
/// `post:<id>`; the bridge adds `option:comments.enabled` and `option:comments.per_page`.
fn comments(scope: &Scope, item: &Value) -> Value {
    let Some(id) = content_id(item) else {
        return Value::from(());
    };
    scope.record(post_key(id));
    if !scope.take_call("comments") {
        return Value::from(());
    }
    let (thread, deps) = scope.bridge.comments(id, scope.cpage);
    scope.merge(deps);
    thread.map_or(Value::from(()), thread_value)
}

/// The template shape of a thread page: `{ open, count, total, page, pages, per_page,
/// prev_url, next_url, items, action, honeypot }` (`count` = `total`, every approved
/// comment of the post; `prev_url`/`next_url` the neighbouring comment pages or none), each
/// item `{ id, parent_id, depth, author_name, author_url, html, date, can_reply }`. `html` is the
/// core-rendered body (escaped plain text in `<p>`/`<br>`), marked safe; everything else
/// is plain data, so auto-escaping applies. `author_url` is handed on only when it is
/// an `http(s)://` URL (core refuses anything else on submit; this guards rows that
/// reach the store another way).
fn thread_value(t: CommentThread) -> Value {
    let items: Vec<Value> = t
        .items
        .into_iter()
        .map(|c| {
            let url = c.author_url.filter(|u| {
                let lower = u.to_ascii_lowercase();
                lower.starts_with("https://") || lower.starts_with("http://")
            });
            minijinja::context! {
                id => c.id.to_string(),
                parent_id => c.parent_id.map(|p| p.to_string()),
                depth => c.depth,
                author_name => c.author_name,
                author_url => url,
                html => Value::from_safe_string(c.html),
                date => c.date.to_rfc3339(),
                can_reply => c.can_reply,
            }
        })
        .collect();
    minijinja::context! {
        open => t.open,
        count => t.total,
        total => t.total,
        page => t.page,
        pages => t.pages,
        per_page => t.per_page,
        prev_url => t.prev_url,
        next_url => t.next_url,
        items => items,
        action => COMMENT_FORM_PATH,
        honeypot => HONEYPOT_FIELD,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_route_helper() {
        assert_eq!(
            plugin_route("contact-form", "/submit"),
            "/_p/contact-form/submit"
        );
        assert_eq!(plugin_route("router", "/"), "/_p/router/");
        assert_eq!(plugin_route("router", "/a/b c"), "/_p/router/a/b%20c");
        assert_eq!(
            plugin_route("router", "/x?y=\"<z>#"),
            "/_p/router/x%3Fy=%22%3Cz%3E%23"
        );
        for (name, path) in [
            ("router", "submit"),
            ("router", ""),
            ("Router", "/x"),
            ("r", "/x"),
            ("router/../api", "/x"),
            ("a b", "/x"),
            ("router", "/../api/v1"),
            ("router", "/a/./b"),
            ("router", "/a/.."),
        ] {
            assert_eq!(plugin_route(name, path), "", "{name:?} {path:?}");
        }
        assert_eq!(plugin_route(&"a".repeat(49), "/x"), "");
        // Usable from templates (a global function); auto-escaped like `asset()`, which
        // browsers decode in attributes.
        let env = build_env(BTreeMap::from([(
            "t.html".to_owned(),
            "<form action=\"{{ plugin_route('contact-form', '/submit') }}\">".to_owned(),
        )]))
        .unwrap();
        let out = env.get_template("t.html").unwrap().render(()).unwrap();
        assert_eq!(
            out,
            "<form action=\"&#x2f;_p&#x2f;contact-form&#x2f;submit\">"
        );
    }

    #[test]
    fn capped_sink_rejects_whole_write() {
        use std::io::Write;
        let mut s = CappedSink {
            buf: Vec::new(),
            cap: 4,
            exceeded: false,
        };
        s.write_all(b"abc").unwrap();
        assert!(s.write_all(b"de").is_err());
        assert!(s.exceeded);
        assert_eq!(s.buf, b"abc");
    }
}
