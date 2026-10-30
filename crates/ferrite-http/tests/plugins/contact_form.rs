//! The `contact-form` example plugin through a real host (M4-3a Task 4, plan P45 and P46):
//! a valid post mails the admins once and lands in the `inbox` blob, a filled honeypot
//! stores and sends nothing, invalid posts get a 422 page, the inbox page is for the listed
//! viewers only, and every abuse in Review Focus 1 is refused or escaped. The route's own
//! limits (the site-origin rule, `10/h`) are checked through the public app.

use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use ferrite_cache::PageCache;
use ferrite_core::{AuthService, PermalinkRules, mail::MemoryMailer};
use ferrite_domain::Role;
use ferrite_http::{AppParts, AppState, ClientIp, HttpConfig, build_app, load_route_table};
use ferrite_plugin_host::{Grant, HostConfig, Manifest, PluginHost, RouteRequest, RouteResponse};
use ferrite_theme::{Theme, ThemeSource};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::analytics::FailingGet;
use crate::common::{SITE_URL, Site, plugin_manifest, plugin_wasm};
use crate::mail::{GRACE, MAIL_SITE_TITLE, WAIT, inside_one_hour_window, mail_services};
use crate::profile_tests;

profile_tests!(
    a_valid_post_mails_the_admins_and_stores_it,
    cr_and_lf_are_stripped_from_name_and_subject,
    a_filled_honeypot_stores_and_sends_nothing,
    a_missing_message_is_422_naming_the_field,
    field_limits_are_checked,
    script_in_name_is_escaped_on_the_error_and_inbox_pages,
    a_foreign_return_falls_back_to_root,
    only_urlencoded_bodies_are_accepted,
    malformed_and_oversized_bodies_are_refused,
    the_eleventh_post_in_an_hour_is_429,
    the_inbox_keeps_the_newest_hundred,
    a_mail_failure_still_thanks_and_stores,
    store_messages_off_mails_only,
    nothing_delivered_is_503,
    the_form_page_posts_to_submit,
    the_inbox_needs_a_listed_viewer,
    the_inbox_is_paged,
    a_failed_read_never_overwrites_the_inbox,
    a_corrupt_inbox_is_replaced_and_a_newer_one_kept,
    worst_cases_fit_half_the_default_fuel,
);

const PLUGIN: &str = "contact-form";
const VIEWER: &str = "viewer-1";
/// What the plugin keeps at most, and the bytes its blob may hold (`INBOX_MAX_BYTES`).
const KEEP: usize = 100;
const INBOX_MAX_BYTES: usize = 900 * 1024;

// ---- helpers --------------------------------------------------------------------------

/// The plugin installed with `fuel` (granted, as an approved install would be), else the
/// default. `CONTACT_FORM_FUEL=<n>` sets it for every case that names none, to check the
/// cases against a lower limit: `CONTACT_FORM_FUEL=20000000 cargo test -p ferrite-http
/// --test plugins contact_form`.
async fn install(site: &Site, fuel: Option<u64>) {
    let fuel = fuel.or_else(|| std::env::var("CONTACT_FORM_FUEL").ok()?.parse().ok());
    match fuel {
        None => site.install(PLUGIN).await,
        Some(fuel) => {
            let manifest = plugin_manifest(PLUGIN) + &format!("\n[limits]\nfuel = {fuel}\n");
            let granted = Grant::of(&Manifest::parse(&manifest).unwrap()).to_json();
            let hash = site.put_artifact(plugin_wasm(PLUGIN)).await;
            site.row_granted(PLUGIN, &manifest, hash, true, granted)
                .await;
        }
    }
    let summary = site.load().await;
    assert_eq!(summary.loaded, [PLUGIN], "{:?}", summary.skipped);
}

/// The site without `[mail]` (every send answers "mail is not configured").
async fn without_mail(site: Site) -> Site {
    install(&site, None).await;
    site
}

/// The site with `[mail]` and one admin, `root`: the mails land in the returned mailer.
async fn with_mail(site: Site) -> (Site, Arc<MemoryMailer>) {
    with_mail_fuel(site, None).await
}

async fn with_mail_fuel(mut site: Site, fuel: Option<u64>) -> (Site, Arc<MemoryMailer>) {
    site.user("root", Role::Admin).await;
    let (services, mailer) = mail_services(&site, MAIL_SITE_TITLE);
    site.services = services.clone();
    site.host = Arc::new(PluginHost::new(HostConfig::default(), services).unwrap());
    install(&site, fuel).await;
    (site, mailer)
}

/// A setting of the plugin (a plain option under its namespace).
async fn setting(site: &Site, key: &str, value: Value) {
    site.store
        .read()
        .options()
        .set(&format!("plugin.{PLUGIN}.settings.{key}"), &value, false)
        .await
        .unwrap();
}

/// `pairs` as an `application/x-www-form-urlencoded` body.
fn form(pairs: &[(&str, &str)]) -> Vec<u8> {
    url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs)
        .finish()
        .into_bytes()
}

/// A valid submission, with `extra` replacing or adding fields.
fn valid<'a>(extra: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
    let mut fields = vec![
        ("name", "Ann Example"),
        ("email", "ann@example.org"),
        ("subject", "Hello there"),
        ("message", "I would like to know more about your work."),
        ("website", ""),
        ("return", "/contact/"),
    ];
    for (k, v) in extra {
        match fields.iter_mut().find(|(fk, _)| fk == k) {
            Some(f) => f.1 = v,
            None => fields.push((k, v)),
        }
    }
    fields
}

fn route(method: &str, path: &str, headers: &[(&str, &str)], body: Vec<u8>) -> RouteRequest {
    route_as(method, path, headers, body, None, &[])
}

fn route_as(
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
    user: Option<&str>,
    query: &[(&str, &str)],
) -> RouteRequest {
    RouteRequest {
        method: method.into(),
        path: path.into(),
        query: query
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect(),
        headers: headers
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect(),
        body,
        client_ip_hash: "h".into(),
        user: user.map(str::to_owned),
    }
}

/// The plugin's answer to `req` (a trap or a deadline fails the case).
async fn call(host: &PluginHost, req: RouteRequest) -> (u16, String) {
    let res: RouteResponse = host.handle_route(PLUGIN, req).await.unwrap();
    let html = res
        .headers
        .iter()
        .any(|(k, v)| k.eq_ignore_ascii_case("content-type") && v == "text/html; charset=utf-8");
    assert!(html, "{:?}", res.headers);
    (res.status, String::from_utf8(res.body).unwrap())
}

/// `POST /submit` of a urlencoded `body`.
async fn submit_raw(site: &Site, body: Vec<u8>) -> (u16, String) {
    let ct = [("content-type", "application/x-www-form-urlencoded")];
    call(&site.host, route("POST", "/submit", &ct, body)).await
}

async fn submit(site: &Site, fields: &[(&str, &str)]) -> (u16, String) {
    submit_raw(site, form(fields)).await
}

/// `GET /inbox` as `user` with `query`.
async fn inbox_page(site: &Site, user: Option<&str>, query: &[(&str, &str)]) -> (u16, String) {
    call(
        &site.host,
        route_as("GET", "/inbox", &[], Vec::new(), user, query),
    )
    .await
}

async fn kv_get(site: &Site) -> Option<Vec<u8>> {
    site.store
        .read()
        .plugin_kv()
        .get(PLUGIN, "inbox")
        .await
        .unwrap()
}

async fn kv_set(site: &Site, value: &[u8]) {
    site.store
        .read()
        .plugin_kv()
        .set(PLUGIN, "inbox", value)
        .await
        .unwrap();
}

/// The stored messages, newest first (empty when nothing is stored): JSON lines, the
/// version line first.
async fn stored(site: &Site) -> Vec<Value> {
    let Some(bytes) = kv_get(site).await else {
        return Vec::new();
    };
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.ends_with('\n'), "{text}");
    let mut lines = text.lines();
    assert_eq!(lines.next(), Some(r#"{"v":1}"#), "{text}");
    lines.map(|l| serde_json::from_str(l).unwrap()).collect()
}

/// A stored message as the plugin writes one.
fn message(n: usize, text: &str) -> Value {
    json!({
        "at": "2026-01-01T00:00:00Z",
        "name": format!("Sender {n}"),
        "email": format!("s{n}@example.org"),
        "subject": format!("Subject {n}"),
        "page": "/contact/",
        "message": text,
    })
}

/// A blob as the plugin writes one, holding `messages` (newest first).
fn blob(messages: Vec<Value>) -> Vec<u8> {
    let mut out = String::from("{\"v\":1}\n");
    for m in messages {
        out.push_str(&m.to_string());
        out.push('\n');
    }
    out.into_bytes()
}

/// The `href` of the page's back link (`<a class="back" href="…">`).
fn back_link(html: &str) -> &str {
    html.split_once("class=\"back\" href=\"")
        .and_then(|(_, rest)| rest.split_once('"'))
        .unwrap_or_else(|| panic!("no back link in {html}"))
        .0
}

/// The thank-you page, as the plugin answers a successful (or honeypot) post.
fn assert_thanks(status: u16, html: &str) {
    assert_eq!(status, 200, "{html}");
    assert!(html.contains("data-page=\"thanks\""), "{html}");
}

/// The public app over `site` (fixture theme), for what the host checks before the
/// plugin: the site-origin rule and the route's rate limit.
async fn app(site: &Site) -> Router {
    let theme_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/app-theme");
    let theme = Arc::new(Theme::load(&ThemeSource::Dir(theme_dir)).unwrap());
    let routes = load_route_table(site.store.as_ref(), &PermalinkRules::default())
        .await
        .unwrap();
    let state = AppState::new(AppParts {
        store: site.store.clone(),
        reader: site.reader.clone(),
        plugins: site.host.clone(),
        media: site.media.clone(),
        theme,
        cache: Arc::new(PageCache::new(1024 * 1024)),
        routes,
        config: HttpConfig::new(SITE_URL, "Test Site", "en"),
        migrations_ok: true,
    })
    .unwrap()
    .with_auth(Arc::new(AuthService::new(
        site.store.clone(),
        site.key.clone(),
    )));
    build_app(Arc::new(state))
}

/// `method path` through `app` from client `ip`, with `headers` and `body`.
async fn send(
    app: &Router,
    method: Method,
    path: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> (StatusCode, axum::http::HeaderMap, String) {
    let mut b = Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, "example.com");
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    let mut req = b.body(Body::from(body)).unwrap();
    req.extensions_mut()
        .insert(ClientIp("203.0.113.9".parse().unwrap()));
    let res = app.clone().oneshot(req).await.unwrap();
    let (parts, body) = res.into_parts();
    let bytes = body.collect().await.unwrap().to_bytes();
    (
        parts.status,
        parts.headers,
        String::from_utf8(bytes.to_vec()).unwrap(),
    )
}

// ---- submissions ----------------------------------------------------------------------

/// P45: one mail to the admins (Bcc), its subject the prefix and the visitor's subject on
/// one line, the body naming the sender, the page and the time; the message is stored.
async fn a_valid_post_mails_the_admins_and_stores_it(site: Site) -> Site {
    let (site, mailer) = with_mail(site).await;
    let before = chrono::Utc::now();
    let (status, html) = submit(&site, &valid(&[])).await;
    assert_thanks(status, &html);
    assert_eq!(back_link(&html), "/contact/");

    let sent = mailer.wait_for(1, WAIT).await;
    assert_eq!(sent.len(), 1, "{sent:?}");
    let mail = &sent[0];
    assert_eq!(mail.bcc, ["root@example.com"]);
    assert_eq!(
        mail.subject,
        format!("[{MAIL_SITE_TITLE}] [Contact] Hello there")
    );
    assert!(!mail.subject.contains(['\r', '\n']));
    for line in [
        "Name: Ann Example\n",
        "E-mail: ann@example.org\n",
        "Subject: Hello there\n",
        "Page: /contact/\n",
        "\nI would like to know more about your work.\n",
    ] {
        assert!(mail.text.contains(line), "{line:?} in {}", mail.text);
    }
    assert_eq!(mailer.wait_for(2, GRACE).await.len(), 1);

    let messages = stored(&site).await;
    assert_eq!(messages.len(), 1);
    let m = &messages[0];
    assert_eq!(m["name"], "Ann Example");
    assert_eq!(m["email"], "ann@example.org");
    assert_eq!(m["subject"], "Hello there");
    assert_eq!(m["page"], "/contact/");
    assert_eq!(m["message"], "I would like to know more about your work.");
    // The wall clock, UTC, RFC 3339 to the second; the mail names the same time.
    let at = m["at"].as_str().unwrap();
    assert!(at.ends_with('Z') && at.len() == 20, "{at}");
    let at_time = chrono::DateTime::parse_from_rfc3339(at).unwrap();
    let after = chrono::Utc::now();
    assert!(
        at_time.timestamp() >= before.timestamp() && at_time.timestamp() <= after.timestamp(),
        "{at} not between {before} and {after}"
    );
    assert!(
        mail.text.contains(&format!("Time: {at}\n")),
        "{}",
        mail.text
    );

    // The subject prefix follows the setting.
    setting(&site, "subject_prefix", json!("Web:")).await;
    submit(&site, &valid(&[("subject", "Second")])).await;
    let sent = mailer.wait_for(2, WAIT).await;
    assert_eq!(sent[1].subject, format!("[{MAIL_SITE_TITLE}] Web: Second"));
    site
}

/// Review Focus 1: CR and LF in `name` and `subject` never reach a header or the stored
/// message; with no subject the mail takes the message's first 60 characters, its line
/// breaks made spaces.
async fn cr_and_lf_are_stripped_from_name_and_subject(site: Site) -> Site {
    let (site, mailer) = with_mail(site).await;
    let (status, html) = submit(
        &site,
        &valid(&[
            ("name", "Ann\r\nBcc: victim@evil.test"),
            ("subject", "Hi\r\nBcc: victim@evil.test\rX-Evil: 1\n"),
        ]),
    )
    .await;
    assert_thanks(status, &html);
    let sent = mailer.wait_for(1, WAIT).await;
    assert_eq!(
        sent[0].subject,
        format!("[{MAIL_SITE_TITLE}] [Contact] HiBcc: victim@evil.testX-Evil: 1")
    );
    assert!(sent[0].text.contains("Name: AnnBcc: victim@evil.test\n"));
    let m = &stored(&site).await[0];
    assert_eq!(m["name"], "AnnBcc: victim@evil.test");
    assert_eq!(m["subject"], "HiBcc: victim@evil.testX-Evil: 1");

    // No subject: the first 60 characters of the message, on one line.
    let text = "Line one\r\nline two\n\nand a third line that goes on well past sixty characters";
    submit(&site, &valid(&[("subject", ""), ("message", text)])).await;
    let sent = mailer.wait_for(2, WAIT).await;
    let first60: String = "Line one line two and a third line that goes on well past sixty"
        .chars()
        .take(60)
        .collect();
    assert_eq!(
        sent[1].subject,
        format!("[{MAIL_SITE_TITLE}] [Contact] {first60}")
    );
    // The message keeps its line breaks (CR LF made LF).
    assert_eq!(
        stored(&site).await[0]["message"],
        "Line one\nline two\n\nand a third line that goes on well past sixty characters"
    );

    // A prefix setting with a line break is made one line too, and the subject stays
    // within the host's 200 characters.
    setting(&site, "subject_prefix", json!("[Site\r\nBcc: x@evil.test]")).await;
    let long = "s".repeat(150);
    submit(&site, &valid(&[("subject", long.as_str())])).await;
    let sent = mailer.wait_for(3, WAIT).await;
    assert_eq!(
        sent[2].subject,
        format!("[{MAIL_SITE_TITLE}] [Site Bcc: x@evil.test] {long}")
    );
    setting(&site, "subject_prefix", json!("p".repeat(300))).await;
    submit(&site, &valid(&[])).await;
    let sent = mailer.wait_for(4, WAIT).await;
    assert_eq!(
        sent[3].subject,
        format!("[{MAIL_SITE_TITLE}] {}", "p".repeat(200))
    );
    site
}

/// P45: a filled honeypot gets the same thank-you page, and nothing is stored or sent.
async fn a_filled_honeypot_stores_and_sends_nothing(site: Site) -> Site {
    let (site, mailer) = with_mail(site).await;
    let (status, html) = submit(&site, &valid(&[("website", "https://spam.test/")])).await;
    assert_thanks(status, &html);
    // Even an otherwise invalid post: a bot is not told what it got wrong.
    let (status, html) = submit(
        &site,
        &[("website", "x"), ("name", ""), ("message", "short")],
    )
    .await;
    assert_thanks(status, &html);
    assert!(kv_get(&site).await.is_none());
    // Prove mail works, then that the honeypot posts sent none.
    submit(&site, &valid(&[])).await;
    assert_eq!(mailer.wait_for(1, WAIT).await.len(), 1);
    assert_eq!(mailer.wait_for(2, GRACE).await.len(), 1);
    assert_eq!(stored(&site).await.len(), 1);
    site
}

/// P45: a missing message is a 422 page naming the field; nothing is stored or sent.
async fn a_missing_message_is_422_naming_the_field(site: Site) -> Site {
    let (site, mailer) = with_mail(site).await;
    let fields: Vec<_> = valid(&[])
        .into_iter()
        .filter(|(k, _)| *k != "message")
        .collect();
    let (status, html) = submit(&site, &fields).await;
    assert_eq!(status, 422, "{html}");
    assert!(html.contains("Message is required."), "{html}");
    // The form comes back filled in, posting to the same route, with a back link.
    assert!(html.contains("value=\"Ann Example\""), "{html}");
    assert!(
        html.contains("action=\"/_p/contact-form/submit\""),
        "{html}"
    );
    assert_eq!(back_link(&html), "/contact/");
    assert!(kv_get(&site).await.is_none());
    submit(&site, &valid(&[])).await;
    assert_eq!(mailer.wait_for(1, WAIT).await.len(), 1);
    assert_eq!(mailer.wait_for(2, GRACE).await.len(), 1);
    site
}

/// P45's limits, each named on the 422 page; several errors are listed together.
async fn field_limits_are_checked(site: Site) -> Site {
    let site = without_mail(site).await;
    let n101 = "n".repeat(101);
    let s151 = "s".repeat(151);
    let m5001 = "m".repeat(5001);
    let e255 = format!("{}@example.org", "e".repeat(243));
    for (field, value, error) in [
        ("name", "", "Name is required."),
        ("name", "  \r\n ", "Name is required."),
        (
            "name",
            n101.as_str(),
            "Name must be at most 100 characters.",
        ),
        ("email", "", "E-mail is required."),
        ("email", "a@b@c.test", "E-mail must be a valid address."),
        (
            "email",
            "no-at.example.org",
            "E-mail must be a valid address.",
        ),
        (
            "email",
            "a b@example.org",
            "E-mail must be a valid address.",
        ),
        (
            "email",
            "a\tb@example.org",
            "E-mail must be a valid address.",
        ),
        ("email", "@x", "E-mail must be a valid address."),
        ("email", e255.as_str(), "E-mail must be a valid address."),
        (
            "subject",
            s151.as_str(),
            "Subject must be at most 150 characters.",
        ),
        (
            "message",
            "too short",
            "Message must be at least 10 characters.",
        ),
        ("message", "   \n\n   ", "Message is required."),
        (
            "message",
            m5001.as_str(),
            "Message must be at most 5000 characters.",
        ),
    ] {
        let (status, html) = submit(&site, &valid(&[(field, value)])).await;
        assert_eq!(status, 422, "{field}={value:?}: {html}");
        assert!(html.contains(error), "{field}={value:?}: {html}");
    }
    let (status, html) = submit(&site, &[("message", "short")]).await;
    assert_eq!(status, 422);
    for error in [
        "Name is required.",
        "E-mail is required.",
        "Message must be at least 10 characters.",
    ] {
        assert!(html.contains(error), "{html}");
    }
    assert!(kv_get(&site).await.is_none());

    // The limits count characters, not bytes; a name of exactly 100 passes.
    let name = "ł".repeat(100);
    let text = "ż".repeat(5000);
    let (status, html) = submit(
        &site,
        &valid(&[("name", name.as_str()), ("message", text.as_str())]),
    )
    .await;
    assert_thanks(status, &html);
    assert_eq!(stored(&site).await[0]["name"], name);
    site
}

/// Review Focus 1: markup in any field comes back escaped, on the 422 page (the form
/// filled in again) and on the inbox page.
async fn script_in_name_is_escaped_on_the_error_and_inbox_pages(site: Site) -> Site {
    let site = without_mail(site).await;
    setting(&site, "viewers", json!(VIEWER)).await;
    let evil = "<script>alert(1)</script>\"'";
    let (status, html) = submit(&site, &valid(&[("name", evil), ("message", "short")])).await;
    assert_eq!(status, 422);
    assert!(!html.contains("<script"), "{html}");
    assert!(
        html.contains("&lt;script&gt;alert(1)&lt;/script&gt;&quot;&#39;"),
        "{html}"
    );

    let (status, _) = submit(
        &site,
        &valid(&[
            ("name", evil),
            ("subject", "<b>bold</b>"),
            ("message", "</textarea><img src=x onerror=alert(1)> hello"),
            ("return", "/a\"><script>/"),
        ]),
    )
    .await;
    assert_eq!(status, 200);
    let (status, html) = inbox_page(&site, Some(VIEWER), &[]).await;
    assert_eq!(status, 200, "{html}");
    assert!(!html.contains("<script"), "{html}");
    assert!(!html.contains("<img"), "{html}");
    assert!(!html.contains("<b>"), "{html}");
    assert!(html.contains("&lt;script&gt;alert(1)"), "{html}");
    assert!(html.contains("&lt;/textarea&gt;&lt;img src=x"), "{html}");
    assert!(html.contains("/a&quot;&gt;&lt;script&gt;/"), "{html}");
    site
}

/// Review Focus 1: a `return` that a browser could read as another origin, or that is not
/// a plain path, gives the back link `/`.
async fn a_foreign_return_falls_back_to_root(site: Site) -> Site {
    let site = without_mail(site).await;
    let long = format!("/{}", "a".repeat(512));
    for bad in [
        "//evil.com",
        "//evil.com/path",
        "/\\evil.com",
        "/ok\\..\\x",
        "javascript:alert(1)",
        "https://evil.com/",
        "evil",
        "/with space",
        "/tab\there",
        "/ünïcode",
        long.as_str(),
    ] {
        let (status, html) = submit(&site, &valid(&[("return", bad)])).await;
        assert_thanks(status, &html);
        assert_eq!(back_link(&html), "/", "{bad:?}");
    }
    // No return at all: `/` too.
    let fields: Vec<_> = valid(&[])
        .into_iter()
        .filter(|(k, _)| *k != "return")
        .collect();
    let (_, html) = submit(&site, &fields).await;
    assert_eq!(back_link(&html), "/");
    // A plain path, query included, is kept (escaped in the attribute).
    let ok = format!("/{}", "a".repeat(511));
    let (_, html) = submit(&site, &valid(&[("return", ok.as_str())])).await;
    assert_eq!(back_link(&html), ok);
    let (_, html) = submit(&site, &valid(&[("return", "/blog/?p=1&q=2")])).await;
    assert_eq!(back_link(&html), "/blog/?p=1&amp;q=2");
    site
}

/// P45: `application/x-www-form-urlencoded` only (any case, with parameters); anything
/// else is 415.
async fn only_urlencoded_bodies_are_accepted(site: Site) -> Site {
    let site = without_mail(site).await;
    let body = form(&valid(&[]));
    for ct in [
        Some("text/plain"),
        Some("multipart/form-data; boundary=x"),
        Some("application/json"),
        Some("application/x-www-form-urlencodedx"),
        None,
    ] {
        let headers: Vec<(&str, &str)> = ct.map(|c| ("content-type", c)).into_iter().collect();
        let (status, html) =
            call(&site.host, route("POST", "/submit", &headers, body.clone())).await;
        assert_eq!(status, 415, "{ct:?}: {html}");
    }
    assert!(kv_get(&site).await.is_none());
    for ct in [
        "application/x-www-form-urlencoded; charset=UTF-8",
        "Application/X-WWW-Form-URLencoded",
    ] {
        let (status, html) = call(
            &site.host,
            route("POST", "/submit", &[("content-type", ct)], body.clone()),
        )
        .await;
        assert_thanks(status, &html);
    }
    site
}

/// Review Focus 1: a body that does not decode is 400 and an oversized one 413, never a
/// trap; nothing is stored.
async fn malformed_and_oversized_bodies_are_refused(site: Site) -> Site {
    let site = without_mail(site).await;
    let many: Vec<String> = (0..101).map(|i| format!("k{i}=v")).collect();
    for body in [
        b"name=%ZZ&message=hello+there+friend".to_vec(),
        b"name=%FF%FE&message=hello+there+friend".to_vec(),
        vec![b'n', b'a', b'm', b'e', b'=', 0xC3, b'&'],
        many.join("&").into_bytes(),
    ] {
        let (status, html) = submit_raw(&site, body).await;
        assert_eq!(status, 400, "{html}");
    }
    // Far more than any valid form (the host passes up to 1 MiB).
    let text = "x".repeat(200_000);
    let big = form(&valid(&[("message", text.as_str())]));
    let (status, html) = submit_raw(&site, big).await;
    assert_eq!(status, 413, "{html}");
    let huge = vec![b'a'; 1024 * 1024];
    let (status, _) = submit_raw(&site, huge).await;
    assert_eq!(status, 413);
    assert!(kv_get(&site).await.is_none());
    site
}

/// The route's `10/h` limit is the host's: through the public app, ten posts in an hour
/// from one client pass and the eleventh is 429. The posts carry `Origin: null`, as the
/// plugin's own (sandboxed) form page sends: `cross_origin` lets them through.
async fn the_eleventh_post_in_an_hour_is_429(site: Site) -> Site {
    let (site, mailer) = with_mail(site).await;
    let app = app(&site).await;
    inside_one_hour_window().await;
    let headers = [
        ("content-type", "application/x-www-form-urlencoded"),
        ("origin", "null"),
    ];
    for i in 0..10 {
        let (status, _, html) = send(
            &app,
            Method::POST,
            "/_p/contact-form/submit",
            &headers,
            form(&valid(&[])),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "post {i}: {html}");
    }
    let (status, headers, _) = send(
        &app,
        Method::POST,
        "/_p/contact-form/submit",
        &headers,
        form(&valid(&[])),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert!(headers.get(header::RETRY_AFTER).is_some());
    assert_eq!(stored(&site).await.len(), 10);
    assert_eq!(mailer.wait_for(10, WAIT).await.len(), 10);
    site
}

/// P45: the inbox holds the latest 100 messages, newest first.
async fn the_inbox_keeps_the_newest_hundred(site: Site) -> Site {
    let site = without_mail(site).await;
    // Two real posts are stored newest first.
    submit(&site, &valid(&[("subject", "first")])).await;
    submit(&site, &valid(&[("subject", "second")])).await;
    let subjects: Vec<Value> = stored(&site)
        .await
        .iter()
        .map(|m| m["subject"].clone())
        .collect();
    assert_eq!(subjects, [json!("second"), json!("first")]);

    // A full inbox (0 the newest, 99 the oldest) takes one more and drops the oldest.
    kv_set(
        &site,
        &blob((0..KEEP).map(|n| message(n, "stored message")).collect()),
    )
    .await;
    let (status, html) = submit(&site, &valid(&[("subject", "newest")])).await;
    assert_thanks(status, &html);
    let messages = stored(&site).await;
    assert_eq!(messages.len(), KEEP);
    assert_eq!(messages[0]["subject"], "newest");
    assert_eq!(messages[1]["subject"], "Subject 0");
    assert_eq!(
        messages[KEEP - 1]["subject"],
        format!("Subject {}", KEEP - 2)
    );
    site
}

/// P45: a mail that cannot be sent (no `[mail]`) is logged; the message is still stored
/// and the visitor still thanked.
async fn a_mail_failure_still_thanks_and_stores(site: Site) -> Site {
    let site = without_mail(site).await;
    let (status, html) = submit(&site, &valid(&[])).await;
    assert_thanks(status, &html);
    assert_eq!(stored(&site).await.len(), 1);
    // Answers, not plugin failures.
    assert!(
        site.host.failures().is_empty(),
        "{:?}",
        site.host.failures()
    );
    site
}

/// `store_messages = false`: the mail goes out, nothing is stored, and the thank-you page
/// shows the `success_message` setting.
async fn store_messages_off_mails_only(site: Site) -> Site {
    let (site, mailer) = with_mail(site).await;
    setting(&site, "store_messages", json!(false)).await;
    setting(
        &site,
        "success_message",
        json!("Thanks!\n<We reply within a day.>"),
    )
    .await;
    let (status, html) = submit(&site, &valid(&[])).await;
    assert_thanks(status, &html);
    assert!(
        html.contains("Thanks!\n&lt;We reply within a day.&gt;"),
        "{html}"
    );
    assert_eq!(mailer.wait_for(1, WAIT).await.len(), 1);
    assert!(kv_get(&site).await.is_none());
    site
}

/// Neither stored (`store_messages = false`) nor mailed (no `[mail]`): the visitor is told
/// it did not go through instead of being thanked.
async fn nothing_delivered_is_503(site: Site) -> Site {
    let site = without_mail(site).await;
    setting(&site, "store_messages", json!(false)).await;
    let (status, html) = submit(&site, &valid(&[])).await;
    assert_eq!(status, 503, "{html}");
    assert!(html.contains("could not be sent"), "{html}");
    assert!(kv_get(&site).await.is_none());
    site
}

// ---- pages ----------------------------------------------------------------------------

/// The standalone form page posts to `/_p/contact-form/submit`, with the honeypot, and
/// is answered with the route containment headers; the site title is the configured one
/// (ruling X1).
async fn the_form_page_posts_to_submit(site: Site) -> Site {
    let (site, _mailer) = with_mail(site).await;
    let app = app(&site).await;
    let (status, headers, html) =
        send(&app, Method::GET, "/_p/contact-form/", &[], Vec::new()).await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert_eq!(
        headers.get(header::CONTENT_SECURITY_POLICY).unwrap(),
        "sandbox allow-forms; form-action 'self'"
    );
    assert!(
        html.contains("<form method=\"post\" action=\"/_p/contact-form/submit\""),
        "{html}"
    );
    for field in ["name", "email", "subject", "message", "website"] {
        assert!(
            html.contains(&format!("name=\"{field}\"")),
            "{field}: {html}"
        );
    }
    assert!(html.contains("Contact Test Site"), "{html}");
    // The stored option wins over the configured title, escaped.
    site.store
        .read()
        .options()
        .set("site.title", &json!("Ann & Co <x>"), true)
        .await
        .unwrap();
    let (_, html) = call(&site.host, route("GET", "/", &[], Vec::new())).await;
    assert!(html.contains("Contact Ann &amp; Co &lt;x&gt;"), "{html}");
    site
}

/// P46: the inbox is for the users listed under `viewers`; anonymous visitors are told to
/// sign in, others are shown their own id.
async fn the_inbox_needs_a_listed_viewer(site: Site) -> Site {
    let site = without_mail(site).await;
    submit(&site, &valid(&[])).await;
    let (status, html) = inbox_page(&site, None, &[]).await;
    assert_eq!(status, 403);
    assert!(html.contains("Sign in to the admin first"), "{html}");
    assert!(!html.contains("Ann Example"), "{html}");
    // Nobody is listed by default.
    let (status, html) = inbox_page(&site, Some(VIEWER), &[]).await;
    assert_eq!(status, 403);
    assert!(html.contains(VIEWER), "{html}");
    assert!(!html.contains("Ann Example"), "{html}");

    setting(&site, "viewers", json!(format!("someone-else,\n{VIEWER}"))).await;
    let (status, html) = inbox_page(&site, Some("other<b>"), &[]).await;
    assert_eq!(status, 403);
    assert!(html.contains("other&lt;b&gt;"), "{html}");
    let (status, html) = inbox_page(&site, Some(VIEWER), &[]).await;
    assert_eq!(status, 200);
    assert!(html.contains("Ann Example"), "{html}");
    assert!(html.contains("ann@example.org"), "{html}");
    assert!(html.contains("I would like to know more"), "{html}");

    // An empty inbox says so.
    site.store
        .read()
        .plugin_kv()
        .delete(PLUGIN, "inbox")
        .await
        .unwrap();
    let (status, html) = inbox_page(&site, Some(VIEWER), &[]).await;
    assert_eq!(status, 200);
    assert!(html.contains("No messages yet."), "{html}");
    site
}

/// The inbox page shows 10 messages a page, newest first; `?page=` is clamped.
async fn the_inbox_is_paged(site: Site) -> Site {
    let site = without_mail(site).await;
    setting(&site, "viewers", json!(VIEWER)).await;
    kv_set(
        &site,
        &blob((0..45).map(|n| message(n, "stored message")).collect()),
    )
    .await;
    let shown = |html: &str| -> Vec<usize> {
        html.split("data-message=\"")
            .skip(1)
            .map(|rest| rest.split('"').next().unwrap().parse().unwrap())
            .collect()
    };
    let (_, html) = inbox_page(&site, Some(VIEWER), &[]).await;
    assert!(html.contains("45 messages"), "{html}");
    assert_eq!(shown(&html), (1..=10).collect::<Vec<_>>());
    assert!(html.contains("Sender 0<"), "{html}");
    assert!(html.contains("href=\"?page=2\""), "{html}");
    assert!(!html.contains("Newer"), "{html}");
    let (_, html) = inbox_page(&site, Some(VIEWER), &[("page", "3")]).await;
    assert_eq!(shown(&html), (21..=30).collect::<Vec<_>>());
    assert!(html.contains("Sender 20<"), "{html}");
    assert!(html.contains("href=\"?page=2\">Newer"), "{html}");
    assert!(html.contains("href=\"?page=4\">Older"), "{html}");
    let (_, html) = inbox_page(&site, Some(VIEWER), &[("page", "5")]).await;
    assert_eq!(shown(&html), (41..=45).collect::<Vec<_>>());
    assert!(html.contains("Sender 44<"), "{html}");
    assert!(!html.contains("Older"), "{html}");
    for (q, first) in [("99", 41), ("0", 1), ("abc", 1), ("-2", 1)] {
        let (status, html) = inbox_page(&site, Some(VIEWER), &[("page", q)]).await;
        assert_eq!(status, 200);
        assert_eq!(shown(&html)[0], first, "page={q}");
    }
    site
}

/// The host answers `None` both for a missing key and for a read that failed: an `inbox`
/// that exists is never overwritten after such a read. The mail still goes out and the
/// visitor is thanked.
async fn a_failed_read_never_overwrites_the_inbox(site: Site) -> Site {
    site.user("root", Role::Admin).await;
    let existing = blob(vec![message(0, "an earlier message")]);
    kv_set(&site, &existing).await;
    let (services, mailer) = mail_services(&site, MAIL_SITE_TITLE);
    let flaky = Arc::new(FailingGet {
        inner: services,
        key: "inbox".into(),
    });
    let host = PluginHost::new(HostConfig::default(), flaky).unwrap();
    host.install(
        Manifest::parse(&plugin_manifest(PLUGIN)).unwrap(),
        &plugin_wasm(PLUGIN),
    )
    .unwrap();
    let ct = [("content-type", "application/x-www-form-urlencoded")];
    let (status, html) = call(&host, route("POST", "/submit", &ct, form(&valid(&[])))).await;
    assert_thanks(status, &html);
    assert_eq!(kv_get(&site).await.unwrap(), existing);
    assert_eq!(mailer.wait_for(1, WAIT).await.len(), 1);
    site
}

/// A blob that is not this plugin's is logged and started again; one written by a newer
/// version is left alone (and not shown); a stored line that is not a message is shown as
/// unreadable and never traps.
async fn a_corrupt_inbox_is_replaced_and_a_newer_one_kept(site: Site) -> Site {
    let (site, mailer) = with_mail(site).await;
    setting(&site, "viewers", json!(VIEWER)).await;
    for corrupt in [
        &b"not json"[..],
        b"",
        b"\n\n",
        br#"[1,2,3]"#,
        br#"{"v":0}"#,
        br#"{"v":"1"}"#,
        br#"{"messages":[]}"#,
    ] {
        kv_set(&site, corrupt).await;
        let (status, html) = inbox_page(&site, Some(VIEWER), &[]).await;
        assert_eq!(status, 200);
        assert!(html.contains("could not be read"), "{html}");
        let (status, html) = submit(&site, &valid(&[])).await;
        assert_thanks(status, &html);
        assert_eq!(
            stored(&site).await.len(),
            1,
            "{:?}",
            String::from_utf8_lossy(corrupt)
        );
    }
    let newer = br#"{"v":2,"messages":[],"other":true}"#;
    kv_set(&site, newer).await;
    let (status, html) = inbox_page(&site, Some(VIEWER), &[]).await;
    assert_eq!(status, 200);
    assert!(html.contains("newer version"), "{html}");
    let (status, html) = submit(&site, &valid(&[])).await;
    assert_thanks(status, &html);
    assert_eq!(kv_get(&site).await.unwrap(), newer);
    assert_eq!(mailer.wait_for(8, WAIT).await.len(), 8);

    // Lines that are not messages: shown as unreadable, kept (newest first) by a submit.
    let mut mixed = blob(vec![message(0, "a readable message")]);
    mixed.extend_from_slice(b"{\"name\":1}\nnot json\n");
    kv_set(&site, &mixed).await;
    let (status, html) = inbox_page(&site, Some(VIEWER), &[]).await;
    assert_eq!(status, 200, "{html}");
    assert!(html.contains("3 messages"), "{html}");
    assert!(html.contains("Sender 0<"), "{html}");
    assert_eq!(html.matches("could not be read").count(), 2, "{html}");
    let (status, html) = submit(&site, &valid(&[])).await;
    assert_thanks(status, &html);
    let text = String::from_utf8(kv_get(&site).await.unwrap()).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 5, "{text}");
    assert!(lines[1].contains("Ann Example"), "{text}");
    assert_eq!(lines[3..], ["{\"name\":1}", "not json"]);
    assert!(
        site.host.failures().is_empty(),
        "{:?}",
        site.host.failures()
    );
    site
}

/// Fuel: the most expensive submit (a full inbox of the largest messages, plus another)
/// and the most expensive inbox page both run at half the default fuel, so the default
/// leaves at least twice what they need. The blob stays within its byte cap, so it also
/// fits the 1 MiB a call may write, and the page fits the 1 MiB response.
async fn worst_cases_fit_half_the_default_fuel(site: Site) -> Site {
    let fuel = HostConfig::default().fuel / 2;
    let (site, _mailer) = with_mail_fuel(site, Some(fuel)).await;
    setting(&site, "viewers", json!(VIEWER)).await;
    // Every character needs an escape in JSON and in HTML (`"` is `\"` and `&quot;`), or
    // takes four bytes: the dearest text per character either way. Plain ASCII fills the
    // inbox with about a hundred.
    for unit in ["\"", "😀", "a"] {
        let text = unit.repeat(5000);
        let name = unit.repeat(100);
        let subject = unit.repeat(150);
        let email = format!("{}@{}", unit.repeat(100), "e".repeat(153));
        let page = format!("/{}", "\"".repeat(511));
        let fields = [
            ("name", name.as_str()),
            ("email", email.as_str()),
            ("subject", subject.as_str()),
            ("message", text.as_str()),
            ("website", ""),
            ("return", page.as_str()),
        ];
        // Fill the inbox through the plugin, until it drops the oldest.
        kv_set(&site, &blob(Vec::new())).await;
        let mut count = 0;
        loop {
            let (status, html) = submit(&site, &fields).await;
            assert_thanks(status, &html);
            let now = stored(&site).await.len();
            if now <= count {
                break;
            }
            count = now;
        }
        let size = kv_get(&site).await.unwrap().len();
        assert!(size <= INBOX_MAX_BYTES, "{unit}: {size} bytes");
        assert!(
            count == KEEP || size > INBOX_MAX_BYTES - 40_000,
            "{unit}: {count} messages, {size} bytes"
        );
        let (status, html) = inbox_page(&site, Some(VIEWER), &[]).await;
        assert_eq!(status, 200);
        assert!(html.len() < 1024 * 1024, "{unit}: {} bytes", html.len());
        eprintln!(
            "contact-form worst case {unit:?}: {count} messages, blob {size} bytes, page {} bytes",
            html.len()
        );
    }
    // A hundred ordinary messages and the 100-message page, at the same fuel.
    kv_set(
        &site,
        &blob(
            (0..KEEP)
                .map(|n| message(n, &"word ".repeat(200)))
                .collect(),
        ),
    )
    .await;
    let (status, html) = submit(&site, &valid(&[])).await;
    assert_thanks(status, &html);
    assert_eq!(stored(&site).await.len(), KEEP);
    let (status, _) = inbox_page(&site, Some(VIEWER), &[("page", "5")]).await;
    assert_eq!(status, 200);
    site
}
