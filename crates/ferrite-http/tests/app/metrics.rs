//! The metrics registry (M4-2 P34): request counts and latencies by route class and
//! status class, page views (no bots, previews or logged-in requests), the page cache
//! and plugin families, and the Prometheus text format of the scrape.

use std::collections::{BTreeMap, BTreeSet};
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use ferrite_http::{
    AppState, ClientIp,
    metrics::{Metrics, MetricsSnapshot},
};

use crate::app_tests;
use crate::harness::{App, request};

const BROWSER: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:131.0) Gecko/20100101 Firefox/131.0";

app_tests!(
    metrics_text_format_is_valid,
    requests_counted_by_class_and_status,
    page_views_skip_bots_previews_logged_in,
    plugin_hook_labels_bounded,
    handler_panic_500_is_counted,
);

/// This app with a fresh state that counts into a registry reading the state's page
/// cache, plugin host and store.
async fn with_metrics(app: App) -> (App, Arc<Metrics>) {
    let app = app
        .rebuilt(App::config(), |s: AppState| {
            let metrics = Metrics::new("9.9.9", "test-target")
                .with_cache(s.cache.clone())
                .with_plugins(s.plugins.clone())
                .with_store(s.store.clone(), Duration::from_secs(30));
            s.with_metrics(Arc::new(metrics))
        })
        .await;
    let metrics = app.state.metrics().expect("metrics").clone();
    (app, metrics)
}

/// GET `path` as a browser (`ua`), optionally with a cookie.
fn browser(path: &str, ua: &str, cookie: Option<&str>) -> Request<Body> {
    let mut b = Request::builder()
        .method(Method::GET)
        .uri(path)
        .header(header::HOST, "example.com")
        .header(header::USER_AGENT, ua);
    if let Some(c) = cookie {
        b = b.header(header::COOKIE, c);
    }
    let mut req = b.body(Body::empty()).expect("request");
    req.extensions_mut()
        .insert(ClientIp("192.0.2.10".parse::<IpAddr>().expect("ip")));
    req
}

/// A series' labels other than `le`, in name order.
type Labels = Vec<(String, String)>;

/// One parsed sample line.
#[derive(Debug, Clone)]
struct Sample {
    name: String,
    labels: BTreeMap<String, String>,
    value: f64,
}

/// A minimal Prometheus text format (0.0.4) check: every line is a `# HELP`, a
/// `# TYPE` or a sample; every sample belongs to a family typed before it (histograms
/// through `_bucket`, `_sum`, `_count`); each family is typed once; histogram buckets
/// grow with `le` and end in `+Inf` equal to `_count`; the text ends in a newline.
fn parse(text: &str) -> Vec<Sample> {
    assert!(text.ends_with('\n'), "ends with a newline");
    let name_ok = |n: &str| {
        let mut chars = n.chars();
        chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == ':')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':')
    };
    let mut types: BTreeMap<String, String> = BTreeMap::new();
    let mut helps: BTreeSet<String> = BTreeSet::new();
    let mut samples = Vec::new();
    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("# HELP ") {
            let (name, _) = rest.split_once(' ').expect("HELP text");
            assert!(name_ok(name), "{line}");
            assert!(helps.insert(name.to_owned()), "HELP twice: {line}");
            continue;
        }
        if let Some(rest) = line.strip_prefix("# TYPE ") {
            let (name, kind) = rest.split_once(' ').expect("TYPE kind");
            assert!(name_ok(name), "{line}");
            assert!(["counter", "gauge", "histogram"].contains(&kind), "{line}");
            assert!(
                types.insert(name.to_owned(), kind.to_owned()).is_none(),
                "TYPE twice: {line}"
            );
            continue;
        }
        assert!(!line.starts_with('#'), "unknown comment: {line}");
        let (head, value) = line.rsplit_once(' ').expect("sample value");
        let value = match value {
            "+Inf" => f64::INFINITY,
            "-Inf" => f64::NEG_INFINITY,
            v => v.parse::<f64>().unwrap_or_else(|_| panic!("value: {line}")),
        };
        let (name, labels) = match head.split_once('{') {
            None => (head, BTreeMap::new()),
            Some((name, rest)) => {
                let body = rest.strip_suffix('}').unwrap_or_else(|| panic!("{line}"));
                (name, parse_labels(body, line))
            }
        };
        assert!(name_ok(name), "{line}");
        let family = ["_bucket", "_sum", "_count"]
            .iter()
            .find_map(|s| {
                name.strip_suffix(s)
                    .filter(|f| types.get(*f).is_some_and(|k| k == "histogram"))
            })
            .unwrap_or(name);
        assert!(types.contains_key(family), "untyped sample: {line}");
        samples.push(Sample {
            name: name.to_owned(),
            labels,
            value,
        });
    }
    // Histogram consistency.
    for (family, kind) in &types {
        if kind != "histogram" {
            continue;
        }
        let bucket = format!("{family}_bucket");
        let count = format!("{family}_count");
        let mut series: BTreeMap<Labels, Vec<(f64, f64)>> = BTreeMap::new();
        for s in samples.iter().filter(|s| s.name == bucket) {
            let le = s.labels.get("le").expect("le label");
            let le = if le == "+Inf" {
                f64::INFINITY
            } else {
                le.parse().expect("le number")
            };
            let key: Vec<_> = s
                .labels
                .iter()
                .filter(|(k, _)| *k != "le")
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            series.entry(key).or_default().push((le, s.value));
        }
        for (key, buckets) in series {
            for pair in buckets.windows(2) {
                assert!(pair[0].0 < pair[1].0, "{family} le order {key:?}");
                assert!(pair[0].1 <= pair[1].1, "{family} cumulative {key:?}");
            }
            let last = buckets.last().expect("buckets");
            assert!(last.0.is_infinite(), "{family} ends in +Inf {key:?}");
            let total = samples
                .iter()
                .find(|s| {
                    s.name == count
                        && s.labels
                            .iter()
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect::<Vec<_>>()
                            == key
                })
                .unwrap_or_else(|| panic!("{family}_count {key:?}"));
            assert_eq!(total.value, last.1, "{family} +Inf = count {key:?}");
        }
    }
    samples
}

fn parse_labels(body: &str, line: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut rest = body;
    while !rest.is_empty() {
        let (key, after) = rest
            .split_once("=\"")
            .unwrap_or_else(|| panic!("label: {line}"));
        let mut value = String::new();
        let mut chars = after.char_indices();
        let end = loop {
            match chars.next() {
                Some((_, '\\')) => match chars.next() {
                    Some((_, 'n')) => value.push('\n'),
                    Some((_, c @ ('\\' | '"'))) => value.push(c),
                    other => panic!("bad escape {other:?}: {line}"),
                },
                Some((i, '"')) => break i,
                Some((_, c)) => value.push(c),
                None => panic!("unterminated label: {line}"),
            }
        };
        assert!(out.insert(key.to_owned(), value).is_none(), "{line}");
        rest = &after[end + 1..];
        rest = rest.strip_prefix(',').unwrap_or(rest);
    }
    out
}

/// The value of `name` with exactly `labels`, if present.
fn value(samples: &[Sample], name: &str, labels: &[(&str, &str)]) -> Option<f64> {
    let want: BTreeMap<String, String> = labels
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    samples
        .iter()
        .find(|s| s.name == name && s.labels == want)
        .map(|s| s.value)
}

async fn metrics_text_format_is_valid(app: App) -> App {
    let (app, metrics) = with_metrics(app).await;
    app.post("Hello", "hello", &[]).await;
    for path in [
        "/",
        "/hello/",
        "/hello/",
        "/missing/",
        "/healthz",
        "/api/v1/x",
    ] {
        app.get(path).await;
    }
    let text = metrics.render().await;
    let samples = parse(&text);
    assert_eq!(
        value(
            &samples,
            "ferrite_build_info",
            &[("version", "9.9.9"), ("target", "test-target")]
        ),
        Some(1.0),
        "{text}"
    );
    // Every P34 family is present.
    for family in [
        "ferrite_build_info",
        "ferrite_http_requests_total",
        "ferrite_http_request_duration_seconds_bucket",
        "ferrite_page_cache_hits_total",
        "ferrite_page_cache_misses_total",
        "ferrite_page_cache_entries",
        "ferrite_page_cache_bytes",
        "ferrite_jobs",
        "ferrite_db_pool_connections",
        "ferrite_nodes",
        "ferrite_page_views_total",
    ] {
        assert!(samples.iter().any(|s| s.name == family), "{family}: {text}");
    }
    // The page latency buckets are P34's.
    let les: Vec<String> = samples
        .iter()
        .filter(|s| {
            s.name == "ferrite_http_request_duration_seconds_bucket"
                && s.labels.get("class").map(String::as_str) == Some("page")
        })
        .map(|s| s.labels["le"].clone())
        .collect();
    assert_eq!(
        les,
        [
            "0.005", "0.01", "0.025", "0.05", "0.1", "0.25", "0.5", "1", "2.5", "5", "10", "+Inf"
        ],
        "{text}"
    );
    // The page cache: one miss then one hit for /hello/.
    assert!(value(&samples, "ferrite_page_cache_hits_total", &[]).unwrap() >= 1.0);
    assert!(value(&samples, "ferrite_page_cache_entries", &[]).unwrap() >= 1.0);
    // Every job status is a series; pool states idle and used.
    for status in ["queued", "running", "done", "dead"] {
        assert!(
            value(&samples, "ferrite_jobs", &[("status", status)]).is_some(),
            "{status}: {text}"
        );
    }
    for state in ["idle", "used"] {
        assert!(
            value(&samples, "ferrite_db_pool_connections", &[("state", state)]).is_some(),
            "{state}: {text}"
        );
    }
    app
}

async fn requests_counted_by_class_and_status(app: App) -> App {
    let (app, metrics) = with_metrics(app).await;
    app.post("Hello", "hello", &[]).await;
    let before = metrics.snapshot();
    assert_eq!(before, MetricsSnapshot::default(), "nothing counted yet");
    let cases: [(Method, &str, &str, &str); 11] = [
        (Method::GET, "/", "page", "2xx"),
        (Method::GET, "/hello/", "page", "2xx"),
        (Method::GET, "/hello", "page", "3xx"),
        (Method::GET, "/no-such-page/", "page", "4xx"),
        (Method::GET, "/api/v1/nothing-here/", "api", "4xx"),
        (Method::GET, "/admin/", "admin", "2xx"),
        (Method::GET, "/_media/00/x.png", "media", "4xx"),
        (Method::GET, "/_p/nobody/", "plugin", "4xx"),
        (Method::GET, "/healthz", "other", "2xx"),
        // The public site's other methods and the comment form are pages, so their 5xx
        // errors show (here: a 405, and a form post refused without its origin).
        (Method::POST, "/", "page", "4xx"),
        (Method::POST, "/_comment", "page", "4xx"),
    ];
    for (method, path, _, _) in &cases {
        let mut req = request(method.clone(), path);
        req.extensions_mut()
            .insert(ClientIp("192.0.2.10".parse::<IpAddr>().unwrap()));
        app.send(req).await;
    }
    let preview = app
        .send(browser(
            "/_preview/0192f0a0-0000-7000-8000-000000000000",
            BROWSER,
            None,
        ))
        .await;
    assert_eq!(preview.status, StatusCode::NOT_FOUND);

    let samples = parse(&metrics.render().await);
    let mut expected: BTreeMap<(&str, &str), f64> = BTreeMap::new();
    for (_, _, class, status) in &cases {
        *expected.entry((class, status)).or_default() += 1.0;
    }
    *expected.entry(("preview", "4xx")).or_default() += 1.0;
    for ((class, status), n) in &expected {
        assert_eq!(
            value(
                &samples,
                "ferrite_http_requests_total",
                &[("class", class), ("status", status)]
            ),
            Some(*n),
            "{class} {status}"
        );
    }
    // Every class × status series exists (zeros included), so rates never start from
    // a missing series.
    let series = samples
        .iter()
        .filter(|s| s.name == "ferrite_http_requests_total")
        .count();
    assert_eq!(series, 8 * 4);
    // Each class's latency count equals its requests.
    for class in [
        "page", "api", "admin", "media", "plugin", "preview", "oauth", "other",
    ] {
        let requests: f64 = samples
            .iter()
            .filter(|s| {
                s.name == "ferrite_http_requests_total"
                    && s.labels.get("class").map(String::as_str) == Some(class)
            })
            .map(|s| s.value)
            .sum();
        assert_eq!(
            value(
                &samples,
                "ferrite_http_request_duration_seconds_count",
                &[("class", class)]
            ),
            Some(requests),
            "{class}"
        );
    }
    let snap = metrics.snapshot();
    // The snapshot leaves out the `other` request, the health check (R3); `/metrics`
    // counts it.
    assert_eq!(snap.requests, 11);
    assert_eq!(snap.errors_5xx, 0);
    // Six page requests: their latencies are in the page histogram.
    assert_eq!(snap.page_latency.iter().sum::<u64>(), 6);
    let delta = snap.since(&before);
    assert_eq!(delta.requests, 11);
    app
}

async fn page_views_skip_bots_previews_logged_in(app: App) -> App {
    let (app, metrics) = with_metrics(app).await;
    let post = app.post("Hello", "hello", &[]).await;
    let draft = app.draft_post("Draft", "draft", &[]).await;
    let views = || metrics.snapshot().page_views;

    // Counted: a 200 HTML page to an anonymous browser (a cache miss and a hit), and a
    // client without a user agent.
    let r = app.send(browser("/hello/", BROWSER, None)).await;
    assert_eq!(r.status, StatusCode::OK);
    app.send(browser("/hello/", BROWSER, None)).await;
    app.get("/").await;
    assert_eq!(views(), 3);

    // Not counted: bots, crawlers, spiders, slurp and link previews (any case).
    for ua in [
        "Mozilla/5.0 (compatible; Googlebot/2.1)",
        "SomeCRAWLER/1.0",
        "Baiduspider/2.0",
        "Mozilla/5.0 (compatible; Yahoo! Slurp)",
        "Mozilla/5.0 (Slack link Preview)",
    ] {
        let r = app.send(browser("/hello/", ua, None)).await;
        assert_eq!(r.status, StatusCode::OK, "{ua}");
    }
    assert_eq!(views(), 3, "bots");

    // Not counted: a logged-in request, a preview, a 404, a redirect, HEAD, a feed (not
    // HTML), its 304, a bot's 304, the API. A browser's 304 revalidation of a page counts
    // (M4-2 R8).
    let cookie = app.session(&app.ada_user).await;
    let r = app.send(browser("/hello/", BROWSER, Some(&cookie))).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(views(), 3, "logged in");
    // Previews, each on its own. Without a session: the shared 404 (never a view).
    // With one: a 200 HTML page, still no view. A 200 preview always needs a session,
    // so no preview can be told apart from a logged-in request by status alone: the
    // handler answers previews before the page-view check (`handle`), and this asserts
    // the outcome.
    let preview = format!("/_preview/{}", draft.id);
    let r = app.send(browser(&preview, BROWSER, None)).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND, "{r:?}");
    let r = app
        .send(browser(&format!("/_preview/{}", post.id), BROWSER, None))
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND, "{r:?}");
    assert_eq!(views(), 3, "anonymous previews");
    let r = app.send(browser(&preview, BROWSER, Some(&cookie))).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(
        r.header("content-type"),
        Some("text/html; charset=utf-8"),
        "{r:?}"
    );
    assert_eq!(views(), 3, "preview");
    let r = app.send(browser("/no-such-page/", BROWSER, None)).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r = app.send(browser("/hello", BROWSER, None)).await;
    assert!(r.status.is_redirection());
    let etag = app.get("/hello/").await.header("etag").unwrap().to_owned();
    assert_eq!(views(), 4, "the plain GET above counts");
    let revalidate = |path: &str, ua: &str, etag: &str, cookie: Option<&str>| {
        let mut cond = browser(path, ua, cookie);
        cond.headers_mut()
            .insert(header::IF_NONE_MATCH, etag.parse().unwrap());
        cond
    };
    let r = app.send(revalidate("/hello/", BROWSER, &etag, None)).await;
    assert_eq!(r.status, StatusCode::NOT_MODIFIED);
    assert_eq!(views(), 5, "a revalidated page view counts");
    let r = app
        .send(revalidate("/hello/", "Googlebot/2.1", &etag, None))
        .await;
    assert_eq!(r.status, StatusCode::NOT_MODIFIED);
    let r = app
        .send(revalidate("/hello/", BROWSER, &etag, Some(&cookie)))
        .await;
    assert_eq!(r.status, StatusCode::NOT_MODIFIED, "{r:?}");
    assert_eq!(views(), 5, "a bot's or a logged-in revalidation");
    let mut head = revalidate("/hello/", BROWSER, &etag, None);
    *head.method_mut() = Method::HEAD;
    assert_eq!(app.send(head).await.status, StatusCode::NOT_MODIFIED);
    let mut head = browser("/hello/", BROWSER, None);
    *head.method_mut() = Method::HEAD;
    assert_eq!(app.send(head).await.status, StatusCode::OK);
    let r = app.send(browser("/feed/", BROWSER, None)).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let feed_etag = r.header("etag").expect("feed etag").to_owned();
    let r = app
        .send(revalidate("/feed/", BROWSER, &feed_etag, None))
        .await;
    assert_eq!(r.status, StatusCode::NOT_MODIFIED, "{r:?}");
    app.send(browser("/api/v1/content", BROWSER, None)).await;
    assert_eq!(views(), 5, "{post:?}");

    let samples = parse(&metrics.render().await);
    assert_eq!(value(&samples, "ferrite_page_views_total", &[]), Some(5.0));
    app
}

async fn plugin_hook_labels_bounded(app: App) -> App {
    let (app, metrics) = with_metrics(app).await;
    app.site.install("router").await;
    app.site.load().await;
    assert!(app.site.host.is_enabled("router"));
    for path in ["/_p/router/hello", "/_p/router/go", "/_p/router/hello"] {
        let r = app.send(browser(path, BROWSER, None)).await;
        assert!(r.status.as_u16() < 500, "{path}: {r:?}");
    }
    let text = metrics.render().await;
    let samples = parse(&text);
    // Route calls are one `route` hook: the route path is not a label.
    assert_eq!(
        value(
            &samples,
            "ferrite_plugin_hook_calls_total",
            &[("plugin", "router"), ("hook", "route")]
        ),
        Some(3.0),
        "{text}"
    );
    assert_eq!(
        value(
            &samples,
            "ferrite_plugin_hook_failures_total",
            &[("plugin", "router"), ("hook", "route")]
        ),
        Some(0.0),
        "{text}"
    );
    assert_eq!(
        value(
            &samples,
            "ferrite_plugin_hook_duration_seconds_count",
            &[("plugin", "router"), ("hook", "route")]
        ),
        Some(3.0),
        "{text}"
    );
    let les: Vec<&str> = samples
        .iter()
        .filter(|s| s.name == "ferrite_plugin_hook_duration_seconds_bucket")
        .map(|s| s.labels["le"].as_str())
        .collect();
    assert_eq!(les, ["0.001", "0.005", "0.025", "0.1", "0.5", "+Inf"]);
    // No label value anywhere holds a path, a query or a URL.
    for s in &samples {
        for (k, v) in &s.labels {
            assert!(
                !v.contains('/') && !v.contains('?') && !v.contains(':'),
                "{} {k}={v:?}",
                s.name
            );
        }
    }
    let snap = metrics.snapshot();
    assert_eq!((snap.plugin_calls, snap.plugin_failures), (3, 0));
    app
}

/// Admin assets whose lookup panics: a handler panic inside the real site stack.
struct PanickingAdmin;

impl ferrite_http::admin::AdminAssets for PanickingAdmin {
    fn get(&self, _path: &str) -> Option<ferrite_http::admin::AdminFile> {
        panic!("admin assets broke")
    }
}

/// A panicking handler's built-in 500 is counted: the metrics layer is outside
/// catch-panic.
async fn handler_panic_500_is_counted(app: App) -> App {
    let app = app
        .rebuilt(App::config(), |s: AppState| {
            let metrics = Metrics::new("9.9.9", "test-target");
            s.with_admin_assets(Arc::new(PanickingAdmin))
                .with_metrics(Arc::new(metrics))
        })
        .await;
    let metrics = app.state.metrics().expect("metrics").clone();
    let r = app.send(browser("/admin/", BROWSER, None)).await;
    assert_eq!(r.status, StatusCode::INTERNAL_SERVER_ERROR, "{r:?}");
    let samples = parse(&metrics.render().await);
    assert_eq!(
        value(
            &samples,
            "ferrite_http_requests_total",
            &[("class", "admin"), ("status", "5xx")]
        ),
        Some(1.0)
    );
    let snap = metrics.snapshot();
    assert_eq!((snap.requests, snap.errors_5xx), (1, 1));
    app
}
