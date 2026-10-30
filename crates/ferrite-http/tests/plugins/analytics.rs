//! The `analytics` example plugin through a real host (M4-3a Task 3, plan P44 and P46):
//! batches of `request.served` events become one KV blob per UTC day, the stats page
//! shows them to the listed viewers only, retention prunes by event time, the maps are
//! capped, and a hostile path or referrer is escaped (Review Focus 2).

use ferrite_plugin_host::{ActionEvent, RouteRequest, RouteResponse};
use serde_json::{Value, json};

use crate::common::Site;
use crate::profile_tests;

profile_tests!(
    a_batch_is_counted_per_day,
    excluded_paths_follow_the_setting,
    stats_page_needs_a_listed_viewer,
    stats_page_escapes_and_truncates,
    stats_page_without_data,
    stats_page_reads_a_full_window,
    a_full_batch_on_full_blobs_fits_the_call_limits,
    days_parameter_sums_the_window,
    retention_keeps_the_newest_days,
    retention_follows_event_time_across_months_and_leap_days,
    retention_setting_is_clamped_and_defaults_to_ninety,
    prune_uses_the_rest_of_the_call_budget,
    maps_are_capped_with_other,
    a_batch_of_many_days_stays_within_the_call_budget,
    the_write_budget_drops_a_day_instead_of_failing,
    stored_blobs_that_are_corrupt_or_newer_are_handled,
    a_malformed_payload_is_refused,
);

const PLUGIN: &str = "analytics";
const D1: &str = "2026-10-29";
const D2: &str = "2026-10-30";
const CAP_PATHS: usize = 500;
const CAP_REFERRERS: usize = 200;

// ---- helpers --------------------------------------------------------------------------

async fn site_with_analytics(site: Site) -> Site {
    let site = crate::with_generous_deadlines(site);
    site.install(PLUGIN).await;
    let summary = site.load().await;
    assert_eq!(summary.loaded, [PLUGIN], "{:?}", summary.skipped);
    site
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

/// A `request.served` event (the host's shape) on `day` at `time` (`HH:MM:SS.ffffff`).
fn event(
    day: &str,
    time: &str,
    path: &str,
    status: u16,
    referrer: Option<&str>,
    ua_class: &str,
) -> Value {
    json!({
        "path": path,
        "status": status,
        "ts": format!("{day}T{time}Z"),
        "referrer_host": referrer,
        "ua_class": ua_class,
        "cache": "miss",
    })
}

/// A browser view of `path` on `day`.
fn view(day: &str, path: &str) -> Value {
    event(day, "12:00:00.000000", path, 200, None, "browser")
}

fn payload(events: Vec<Value>) -> ActionEvent {
    ActionEvent {
        subject: None,
        payload: json!({ "events": events }),
    }
}

async fn try_feed(site: &Site, events: Vec<Value>) -> Result<(), ferrite_plugin_host::PluginError> {
    site.host
        .dispatch_action(PLUGIN, "request.served", payload(events))
        .await
}

async fn feed(site: &Site, events: Vec<Value>) {
    try_feed(site, events).await.unwrap();
}

async fn kv_get(site: &Site, key: &str) -> Option<Vec<u8>> {
    site.store
        .read()
        .plugin_kv()
        .get(PLUGIN, key)
        .await
        .unwrap()
}

async fn kv_set(site: &Site, key: &str, value: &[u8]) {
    site.store
        .read()
        .plugin_kv()
        .set(PLUGIN, key, value)
        .await
        .unwrap();
}

/// The decoded blob of `day`.
async fn day(site: &Site, day: &str) -> Option<Value> {
    kv_get(site, &format!("day:{day}"))
        .await
        .map(|b| serde_json::from_slice(&b).unwrap())
}

/// The stored day keys without their prefix, ascending.
async fn days(site: &Site) -> Vec<String> {
    site.store
        .read()
        .plugin_kv()
        .list_keys(PLUGIN, "day:", 200)
        .await
        .unwrap()
        .into_iter()
        .map(|k| k.strip_prefix("day:").unwrap().to_owned())
        .collect()
}

async fn latest(site: &Site) -> Option<String> {
    kv_get(site, "latest")
        .await
        .map(|b| String::from_utf8(b).unwrap())
}

fn request(user: Option<&str>, query: &[(&str, &str)]) -> RouteRequest {
    RouteRequest {
        method: "GET".into(),
        path: "/".into(),
        query: query
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect(),
        headers: Vec::new(),
        body: Vec::new(),
        client_ip_hash: "h".into(),
        user: user.map(str::to_owned),
    }
}

async fn page_as(site: &Site, user: Option<&str>, query: &[(&str, &str)]) -> RouteResponse {
    site.host
        .handle_route(PLUGIN, request(user, query))
        .await
        .unwrap()
}

/// The stats page for the listed viewer `v`.
async fn stats(site: &Site, query: &[(&str, &str)]) -> (u16, String) {
    let res = page_as(site, Some("v"), query).await;
    (res.status, String::from_utf8(res.body).unwrap())
}

/// A site whose only viewer is `v`.
async fn viewer_site(site: Site) -> Site {
    let site = site_with_analytics(site).await;
    setting(&site, "viewers", json!("v")).await;
    site
}

/// The number in the page's `data-stat="name"` element.
fn stat(html: &str, name: &str) -> u64 {
    let marker = format!("data-stat=\"{name}\">");
    let rest = html
        .split_once(&marker)
        .unwrap_or_else(|| panic!("no {name} in {html}"))
        .1;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits
        .parse()
        .unwrap_or_else(|_| panic!("no number after {marker}: {rest:.40}"))
}

/// `2026-09-01` plus `n` days, for `n` below 60.
fn date(n: u32) -> String {
    if n < 30 {
        format!("2026-09-{:02}", n + 1)
    } else {
        format!("2026-10-{:02}", n - 29)
    }
}

// ---- counting -------------------------------------------------------------------------

/// The brief's batch: two days (across midnight), a bot, a client of another kind, a 404,
/// other status classes, a self-referral, excluded paths, and two events the plugin
/// cannot use. Review Focus 2: a batch spanning midnight writes two blobs.
async fn a_batch_is_counted_per_day(site: Site) -> Site {
    let site = site_with_analytics(site).await;
    let news = Some("news.example.org");
    feed(
        &site,
        vec![
            event(D1, "08:00:00.000000", "/", 200, news, "browser"),
            event(D1, "08:00:01.000000", "/a/", 200, None, "browser"),
            event(D1, "08:00:02.000000", "/a/", 304, news, "browser"),
            event(D1, "08:00:03.000000", "/missing/", 404, None, "browser"),
            event(D1, "08:00:04.000000", "/", 200, None, "bot"),
            event(D1, "08:00:05.000000", "/feed/", 200, None, "other"),
            event(
                D1,
                "08:00:06.000000",
                "/x/",
                200,
                Some("example.com"),
                "browser",
            ),
            event(D1, "08:00:07.000000", "/admin/users", 200, None, "browser"),
            event(D1, "08:00:08.000000", "/_p/foo", 200, None, "browser"),
            event(D1, "23:59:59.999999", "/go/", 301, None, "browser"),
            event(
                D2,
                "00:00:00.000000",
                "/",
                200,
                Some("search.example.net"),
                "browser",
            ),
            event(D2, "00:00:01.000000", "/boom/", 500, None, "browser"),
            event(D2, "00:00:02.000000", "/", 200, None, "browser"),
            // Unusable: a time that is not a time, and an event of the wrong shape.
            json!({"path": "/", "status": 200, "ts": "garbage", "referrer_host": null,
                   "ua_class": "browser", "cache": "hit"}),
            json!({"path": 7}),
        ],
    )
    .await;

    assert_eq!(
        day(&site, D1).await.unwrap(),
        json!({
            "v": 1, "views": 4, "bots": 1, "other": 1,
            "status": {"2xx": 5, "3xx": 2, "4xx": 1},
            "paths": {"/": 1, "/a/": 2, "/x/": 1},
            "referrers": {"news.example.org": 2},
        })
    );
    assert_eq!(
        day(&site, D2).await.unwrap(),
        json!({
            "v": 1, "views": 2, "bots": 0, "other": 0,
            "status": {"2xx": 2, "5xx": 1},
            "paths": {"/": 2},
            "referrers": {"search.example.net": 1},
        })
    );
    assert_eq!(days(&site).await, [D1, D2]);
    assert_eq!(latest(&site).await.as_deref(), Some(D2));

    // A second batch adds to the stored blobs.
    feed(
        &site,
        vec![
            event(D2, "01:00:00.000000", "/", 200, news, "browser"),
            event(D2, "01:00:01.000000", "/new/", 404, None, "browser"),
        ],
    )
    .await;
    let d2 = day(&site, D2).await.unwrap();
    assert_eq!(d2["views"], 3);
    assert_eq!(d2["status"], json!({"2xx": 3, "4xx": 1, "5xx": 1}));
    assert_eq!(d2["paths"], json!({"/": 3}));
    assert_eq!(
        d2["referrers"],
        json!({"news.example.org": 1, "search.example.net": 1})
    );
    assert_eq!(days(&site).await, [D1, D2]);
    site
}

async fn excluded_paths_follow_the_setting(site: Site) -> Site {
    let site = site_with_analytics(site).await;
    // The default, "/admin,/_p": whole segments only.
    feed(
        &site,
        [
            "/admin",
            "/admin/",
            "/admin/x",
            "/administrator",
            "/_p/x",
            "/adm",
            "/a",
        ]
        .into_iter()
        .map(|p| view(D1, p))
        .collect(),
    )
    .await;
    assert_eq!(
        day(&site, D1).await.unwrap()["paths"],
        json!({"/administrator": 1, "/adm": 1, "/a": 1})
    );

    // Own prefixes, spaces around them and entries that are not paths are tolerated.
    setting(&site, "exclude_paths", json!(" /private , ,nope, /tmp/ ")).await;
    feed(
        &site,
        [
            "/private",
            "/private/x",
            "/privately",
            "/tmp",
            "/tmp/y",
            "/admin/z",
            "/nope",
        ]
        .into_iter()
        .map(|p| view(D2, p))
        .collect(),
    )
    .await;
    assert_eq!(
        day(&site, D2).await.unwrap()["paths"],
        json!({"/privately": 1, "/admin/z": 1, "/nope": 1})
    );

    // An empty setting excludes nothing.
    setting(&site, "exclude_paths", json!("")).await;
    feed(&site, vec![view("2026-10-31", "/admin/users")]).await;
    assert_eq!(
        day(&site, "2026-10-31").await.unwrap()["paths"],
        json!({"/admin/users": 1})
    );
    site
}

// ---- the stats page -------------------------------------------------------------------

/// P46: anonymous is refused with a pointer to the admin; a signed-in user who is not
/// listed sees their own id; an empty or absent `viewers` allows nobody; ids compare
/// exactly.
async fn stats_page_needs_a_listed_viewer(site: Site) -> Site {
    let site = site_with_analytics(site).await;
    feed(&site, vec![view(D1, "/")]).await;

    // `viewers` not set at all.
    let res = page_as(&site, None, &[]).await;
    assert_eq!(res.status, 403);
    let body = String::from_utf8(res.body).unwrap();
    assert!(body.contains("Sign in to the admin first"), "{body}");
    assert!(!body.contains("data-stat"), "no figures: {body}");

    let res = page_as(&site, Some("user-1"), &[]).await;
    assert_eq!(res.status, 403);
    let body = String::from_utf8(res.body).unwrap();
    assert!(body.contains("user-1"), "{body}");
    assert!(body.contains("Viewers"), "{body}");
    assert!(!body.contains("data-stat"), "no figures: {body}");

    // The user id is echoed escaped.
    let res = page_as(&site, Some("<b>x</b>"), &[]).await;
    assert_eq!(res.status, 403);
    let body = String::from_utf8(res.body).unwrap();
    assert!(body.contains("&lt;b&gt;x&lt;/b&gt;"), "{body}");
    assert!(!body.contains("<b>x</b>"), "{body}");

    // An empty list allows nobody.
    setting(&site, "viewers", json!("")).await;
    assert_eq!(page_as(&site, Some("user-1"), &[]).await.status, 403);

    // Listed, with commas, new lines and spaces; the match is exact.
    setting(&site, "viewers", json!("user-10,\n user-2\tuser-3 ")).await;
    assert_eq!(page_as(&site, Some("user-1"), &[]).await.status, 403);
    assert_eq!(page_as(&site, Some("user-"), &[]).await.status, 403);
    assert_eq!(page_as(&site, Some("User-2"), &[]).await.status, 403);
    assert_eq!(page_as(&site, None, &[]).await.status, 403);
    for id in ["user-10", "user-2", "user-3"] {
        let res = page_as(&site, Some(id), &[]).await;
        assert_eq!(res.status, 200, "{id}");
        let html = res
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
            .map(|(_, v)| v.as_str());
        assert_eq!(html, Some("text/html; charset=utf-8"));
    }
    site
}

/// Review Focus 2: a path or referrer with markup, quotes, or a very long string is
/// escaped and truncated; the page holds no script.
async fn stats_page_escapes_and_truncates(site: Site) -> Site {
    let site = viewer_site(site).await;
    let hostile = "/<script>alert(1)</script>?\"'&x=<b>";
    let long = format!("/{}", "a".repeat(600));
    let host = "\"><img src=x onerror=alert(1)>";
    feed(
        &site,
        vec![
            event(D1, "10:00:00.000000", hostile, 200, Some(host), "browser"),
            event(
                D1,
                "10:00:01.000000",
                &long,
                200,
                Some(&"h".repeat(400)),
                "browser",
            ),
        ],
    )
    .await;
    let (status, html) = stats(&site, &[]).await;
    assert_eq!(status, 200);
    assert!(!html.contains("<script"), "{html}");
    assert!(!html.contains("<img"), "{html}");
    assert!(!html.contains("<b>"), "{html}");
    assert!(
        html.contains("&lt;script&gt;alert(1)&lt;/script&gt;?&quot;&#39;&amp;x=&lt;b&gt;"),
        "{html}"
    );
    assert!(
        html.contains("&quot;&gt;&lt;img src=x onerror=alert(1)&gt;"),
        "{html}"
    );
    // 200 characters of a long path or host, then an ellipsis.
    assert!(html.contains(&format!("/{}…", "a".repeat(199))), "{html}");
    assert!(!html.contains(&"a".repeat(200)), "{html}");
    assert!(html.contains(&format!("{}…", "h".repeat(200))), "{html}");
    assert!(!html.contains(&"h".repeat(201)), "{html}");
    // Totals, a chart without script, and the two tables.
    assert_eq!(stat(&html, "views"), 2);
    assert!(html.contains("<svg"), "{html}");
    site
}

async fn stats_page_without_data(site: Site) -> Site {
    let site = viewer_site(site).await;
    let (status, html) = stats(&site, &[]).await;
    assert_eq!(status, 200);
    assert!(html.contains("No data yet"), "{html}");
    assert!(!html.contains("data-stat"), "{html}");
    site
}

/// The 90 days a page can show, each with 500 paths and 200 referrers: the stored blobs
/// the page reads are the biggest the plugin writes in practice, and the call stays
/// within its fuel (the default limit; measured at half of it) and its 100 KV operations.
async fn stats_page_reads_a_full_window(site: Site) -> Site {
    let site = viewer_site(site).await;
    let paths: serde_json::Map<String, Value> = (0..CAP_PATHS)
        .map(|i| {
            (
                format!("/blog/2026/a-reasonably-long-post-title-{i:03}/"),
                json!(3),
            )
        })
        .collect();
    let referrers: serde_json::Map<String, Value> = (0..CAP_REFERRERS)
        .map(|i| (format!("referrer-{i:03}.example.org"), json!(5)))
        .collect();
    // In the layout the plugin writes: the counts first, then `paths` and `referrers`.
    let blob = format!(
        r#"{{"v":1,"views":1500,"bots":10,"other":5,"status":{{"2xx":1600,"4xx":7}},"paths":{},"referrers":{}}}"#,
        Value::Object(paths),
        Value::Object(referrers)
    )
    .into_bytes();
    assert!(blob.len() > 30_000, "{}", blob.len());
    let mut stored = Vec::new();
    for (month, last) in [(7, 31), (8, 31), (9, 30)] {
        stored.extend((1..=last).map(|d| format!("2026-{month:02}-{d:02}")));
    }
    stored.truncate(90);
    for d in &stored {
        kv_set(&site, &format!("day:{d}"), &blob).await;
    }
    kv_set(&site, "latest", stored.last().unwrap().as_bytes()).await;

    let (status, html) = stats(&site, &[("days", "90")]).await;
    assert_eq!(status, 200, "{}", &html[..html.len().min(300)]);
    // Every day is in the totals and the chart; the lists cover the newest days whose
    // blobs fit the decode budget (200 000 bytes), and the page says so.
    let listed = 200_000 / blob.len();
    assert_eq!(stat(&html, "views"), 90 * 1500);
    assert_eq!(stat(&html, "bots"), 90 * 10);
    assert_eq!(html.matches("class=\"bar\"").count(), 90);
    assert!(
        html.contains(&format!(
            "cover the newest {listed} of the 90 days with data"
        )),
        "{html}"
    );
    assert!(html.contains("referrer-000.example.org"));
    assert!(html.contains("/blog/2026/a-reasonably-long-post-title-000/"));

    // A shorter period fits entirely: no note.
    let (status, html) = stats(&site, &[("days", "5")]).await;
    assert_eq!(status, 200);
    assert!(!html.contains("data-note"), "{html}");
    site
}

/// A saturated blob (500 paths, 200 referrers) in the layout the plugin writes.
fn full_blob(prefix: &str) -> Vec<u8> {
    let map = |n: usize, key: &dyn Fn(usize) -> String| {
        Value::Object((0..n).map(|i| (key(i), json!(3))).collect())
    };
    format!(
        r#"{{"v":1,"views":1500,"bots":0,"other":0,"status":{{"2xx":1500}},"paths":{},"referrers":{}}}"#,
        map(CAP_PATHS, &|i| format!("/{prefix}/a-reasonably-long-post-title-{i:03}/")),
        map(CAP_REFERRERS, &|i| format!("referrer-{i:03}.example.org")),
    )
    .into_bytes()
}

/// The biggest batch (1000 events, across midnight) on two saturated stored days: decode,
/// count and encode both blobs inside the default fuel and the 1 MiB write budget.
async fn a_full_batch_on_full_blobs_fits_the_call_limits(site: Site) -> Site {
    let site = site_with_analytics(site).await;
    for d in [D1, D2] {
        kv_set(&site, &format!("day:{d}"), &full_blob("blog")).await;
    }
    let events: Vec<Value> = (0..1000)
        .map(|i| {
            let d = if i % 2 == 0 { D1 } else { D2 };
            let path = format!("/blog/a-reasonably-long-post-title-{:03}/", i % CAP_PATHS);
            let referrer = format!("referrer-{:03}.example.org", i % CAP_REFERRERS);
            event(d, "12:00:00.000000", &path, 200, Some(&referrer), "browser")
        })
        .collect();
    feed(&site, events).await;
    // Day 1 got the even events, day 2 the odd ones; each path of the stored blob that an
    // event named (twice) went from 3 to 5.
    for (d, hit) in [(D1, "000"), (D2, "001")] {
        let blob = day(&site, d).await.unwrap();
        assert_eq!(blob["views"], 2000, "{d}");
        let paths = blob["paths"].as_object().unwrap();
        assert_eq!(
            paths.len(),
            CAP_PATHS,
            "no new key: nothing goes to (other)"
        );
        assert_eq!(
            paths[&format!("/blog/a-reasonably-long-post-title-{hit}/")],
            5
        );
    }
    site
}

/// `?days=N` sums the N days ending at the newest stored day; an unusable N means 30 and
/// a large one is capped at 90.
async fn days_parameter_sums_the_window(site: Site) -> Site {
    let site = viewer_site(site).await;
    // Day i (0..40) has i + 1 views, one batch per day.
    for i in 0..40u32 {
        let d = date(i);
        feed(
            &site,
            (0..=i)
                .map(|_| view(&d, "/"))
                .chain([event(&d, "13:00:00.000000", "/bot/", 200, None, "bot")])
                .collect(),
        )
        .await;
    }
    assert_eq!(latest(&site).await.as_deref(), Some(date(39).as_str()));

    let sum = |from: u32| (from..40).map(|i| u64::from(i) + 1).sum::<u64>();
    for (query, days, views, bots) in [
        (vec![("days", "1")], 1, 40, 1),
        (vec![("days", "7")], 7, sum(33), 7),
        (vec![], 30, sum(10), 30),
        (vec![("days", "30")], 30, sum(10), 30),
        (vec![("days", "90")], 90, sum(0), 40),
        (vec![("days", "0")], 1, 40, 1),
        (vec![("days", "abc")], 30, sum(10), 30),
        (vec![("days", "-3")], 30, sum(10), 30),
        (vec![("days", "500")], 90, sum(0), 40),
        (vec![("days", "7"), ("days", "1")], 7, sum(33), 7),
    ] {
        let (status, html) = stats(&site, &query).await;
        assert_eq!(status, 200, "{query:?}");
        assert_eq!(stat(&html, "days"), days, "{query:?}");
        assert_eq!(stat(&html, "views"), views, "{query:?}");
        assert_eq!(stat(&html, "bots"), bots, "{query:?}");
        // One bar per day of the window, empty days included.
        assert_eq!(
            html.matches("class=\"bar\"").count() as u64,
            days,
            "{query:?}"
        );
    }
    site
}

// ---- retention ------------------------------------------------------------------------

/// "Keep N days" keeps the newest N, whatever the order the batches arrive in; an event
/// older than that is not stored.
async fn retention_keeps_the_newest_days(site: Site) -> Site {
    let site = site_with_analytics(site).await;
    setting(&site, "retention_days", json!(3)).await;
    for d in ["2026-12-29", "2026-12-30", "2026-12-31"] {
        feed(&site, vec![view(d, "/")]).await;
    }
    assert_eq!(
        days(&site).await,
        ["2026-12-29", "2026-12-30", "2026-12-31"]
    );
    feed(&site, vec![view("2027-01-01", "/")]).await;
    assert_eq!(
        days(&site).await,
        ["2026-12-30", "2026-12-31", "2027-01-01"]
    );
    feed(&site, vec![view("2027-01-02", "/")]).await;
    assert_eq!(
        days(&site).await,
        ["2026-12-31", "2027-01-01", "2027-01-02"]
    );
    // A straggler from before the window is dropped, not stored and pruned later.
    feed(&site, vec![view("2026-12-29", "/old/")]).await;
    assert_eq!(
        days(&site).await,
        ["2026-12-31", "2027-01-01", "2027-01-02"]
    );
    // The latest day marker is not a day key and stays.
    assert_eq!(latest(&site).await.as_deref(), Some("2027-01-02"));

    // Retention 1: only the newest day.
    setting(&site, "retention_days", json!(1)).await;
    feed(&site, vec![view("2027-01-03", "/")]).await;
    assert_eq!(days(&site).await, ["2027-01-03"]);
    // A batch of several days: the older ones are dropped within the batch.
    feed(
        &site,
        vec![
            view("2027-01-03", "/"),
            view("2027-01-04", "/"),
            view("2027-01-05", "/"),
        ],
    )
    .await;
    assert_eq!(days(&site).await, ["2027-01-05"]);
    site
}

/// The cutoff is calendar arithmetic: it crosses a year, and a leap day exists only in
/// leap years.
async fn retention_follows_event_time_across_months_and_leap_days(site: Site) -> Site {
    let site = site_with_analytics(site).await;
    setting(&site, "retention_days", json!(2)).await;
    for d in ["2027-02-27", "2027-02-28", "2027-03-01"] {
        feed(&site, vec![view(d, "/")]).await;
    }
    // 2027 has no 29 February: the day before 1 March is the 28th.
    assert_eq!(days(&site).await, ["2027-02-28", "2027-03-01"]);

    for d in ["2028-02-28", "2028-02-29", "2028-03-01"] {
        feed(&site, vec![view(d, "/")]).await;
    }
    assert_eq!(days(&site).await, ["2028-02-29", "2028-03-01"]);

    // 2100 is not a leap year; 2000 is.
    for d in ["2100-02-28", "2100-03-01"] {
        feed(&site, vec![view(d, "/")]).await;
    }
    assert_eq!(days(&site).await, ["2100-02-28", "2100-03-01"]);
    // An impossible date is an unusable event.
    feed(
        &site,
        vec![view("2100-02-29", "/"), view("2100-13-01", "/")],
    )
    .await;
    assert_eq!(days(&site).await, ["2100-02-28", "2100-03-01"]);
    site
}

/// Valid is 1..=366: more is 366, less is 1, and an unset or unusable value is 90.
async fn retention_setting_is_clamped_and_defaults_to_ninety(site: Site) -> Site {
    let site = site_with_analytics(site).await;
    let blob = br#"{"v":1,"views":1}"#;

    // Unset: the manifest default, 90 days. The newest day is 2026-10-30, so 2026-08-02
    // (89 days earlier) is the oldest kept.
    for d in ["2026-08-01", "2026-08-02"] {
        kv_set(&site, &format!("day:{d}"), blob).await;
    }
    feed(&site, vec![view(D2, "/")]).await;
    assert_eq!(days(&site).await, ["2026-08-02", D2]);

    // Not a number: 90 again.
    setting(&site, "retention_days", json!("many")).await;
    kv_set(&site, "day:2026-08-01", blob).await;
    feed(&site, vec![view(D2, "/")]).await;
    assert_eq!(days(&site).await, ["2026-08-02", D2]);

    // 5000 is 366: 2025-10-30 (365 days before the newest) is kept, the day before goes.
    setting(&site, "retention_days", json!(5000)).await;
    for d in ["2025-10-29", "2025-10-30"] {
        kv_set(&site, &format!("day:{d}"), blob).await;
    }
    feed(&site, vec![view(D2, "/")]).await;
    assert_eq!(days(&site).await, ["2025-10-30", "2026-08-02", D2]);

    // 0 and negative are 1.
    for v in [json!(0), json!(-7)] {
        setting(&site, "retention_days", v).await;
        kv_set(&site, "day:2026-10-29", blob).await;
        feed(&site, vec![view(D2, "/")]).await;
        assert_eq!(days(&site).await, [D2]);
        kv_set(&site, "day:2026-08-02", blob).await;
    }
    site
}

/// A prune deletes with what is left of the 100 operations of the call, and the next
/// batches finish the job.
async fn prune_uses_the_rest_of_the_call_budget(site: Site) -> Site {
    let site = site_with_analytics(site).await;
    setting(&site, "retention_days", json!(5)).await;
    let mut old = Vec::new();
    for month in 1..=12 {
        for d in 1..=28 {
            old.push(format!("2020-{month:02}-{d:02}"));
        }
    }
    old.truncate(130);
    for d in &old {
        kv_set(&site, &format!("day:{d}"), br#"{"v":1}"#).await;
    }
    // Five operations go to counting (the latest day, the day's blob in and out, the
    // latest day out, the listing): 95 deletes of the 130 old days.
    feed(&site, vec![view(D2, "/")]).await;
    let left = days(&site).await;
    assert_eq!(left.len(), 130 - 95 + 1, "{left:?}");
    assert!(left.contains(&D2.to_owned()));
    // The oldest go first.
    assert_eq!(left[0], old[95]);

    feed(&site, vec![view(D2, "/")]).await;
    assert_eq!(days(&site).await, [D2]);
    assert_eq!(day(&site, D2).await.unwrap()["views"], 2);
    site
}

// ---- caps and budgets -----------------------------------------------------------------

/// P44: `paths` and `referrers` are capped at 500 and 200 keys; past that, new keys count
/// under "(other)". Keys already stored keep counting.
async fn maps_are_capped_with_other(site: Site) -> Site {
    let site = site_with_analytics(site).await;
    let events: Vec<Value> = (0..520)
        .map(|i| {
            let referrer = (i < 220).then(|| format!("h{i:03}.example.org"));
            event(
                D1,
                "09:00:00.000000",
                &format!("/p{i:03}"),
                200,
                referrer.as_deref(),
                "browser",
            )
        })
        .collect();
    feed(&site, events).await;

    let blob = day(&site, D1).await.unwrap();
    assert_eq!(blob["views"], 520);
    let paths = blob["paths"].as_object().unwrap();
    assert_eq!(paths.len(), CAP_PATHS + 1);
    assert_eq!(paths["(other)"], 20);
    assert_eq!(paths["/p000"], 1);
    assert_eq!(paths["/p499"], 1);
    assert!(!paths.contains_key("/p500"));
    let referrers = blob["referrers"].as_object().unwrap();
    assert_eq!(referrers.len(), CAP_REFERRERS + 1);
    assert_eq!(referrers["(other)"], 20);
    assert!(referrers.contains_key("h199.example.org"));
    assert!(!referrers.contains_key("h200.example.org"));

    // A stored key keeps its own count; a new key still goes to "(other)".
    feed(
        &site,
        vec![
            event(
                D1,
                "09:01:00.000000",
                "/p000",
                200,
                Some("h000.example.org"),
                "browser",
            ),
            event(
                D1,
                "09:01:01.000000",
                "/new",
                200,
                Some("new.example.org"),
                "browser",
            ),
        ],
    )
    .await;
    let blob = day(&site, D1).await.unwrap();
    assert_eq!(blob["views"], 522);
    assert_eq!(blob["paths"]["/p000"], 2);
    assert_eq!(blob["paths"]["(other)"], 21);
    assert_eq!(blob["paths"].as_object().unwrap().len(), CAP_PATHS + 1);
    assert_eq!(blob["referrers"]["h000.example.org"], 2);
    assert_eq!(blob["referrers"]["(other)"], 21);
    assert!(
        kv_get(&site, &format!("day:{D1}")).await.unwrap().len() < 1024 * 1024,
        "under the per-call write budget"
    );

    // The page ranks real paths and shows "(other)" last.
    setting(&site, "viewers", json!("v")).await;
    let (status, html) = stats(&site, &[]).await;
    assert_eq!(status, 200);
    assert_eq!(stat(&html, "views"), 522);
    assert!(html.contains("(other)"), "{html}");
    site
}

/// Ten days in one batch (a delayed or replayed job): the newest eight are counted and
/// the call stays within its 100 KV operations, so it does not fail.
async fn a_batch_of_many_days_stays_within_the_call_budget(site: Site) -> Site {
    let site = site_with_analytics(site).await;
    let events: Vec<Value> = (0..10).map(|i| view(&date(i), "/")).collect();
    feed(&site, events).await;
    let want: Vec<String> = (2..10).map(date).collect();
    assert_eq!(days(&site).await, want);
    assert_eq!(latest(&site).await.as_deref(), Some(date(9).as_str()));
    site
}

/// Writes are limited to 1 MiB per call. With large stored blobs the oldest day of a
/// batch is dropped (and logged) rather than failing the whole call.
async fn the_write_budget_drops_a_day_instead_of_failing(site: Site) -> Site {
    let site = site_with_analytics(site).await;
    // ~450 KB each: 500 paths of ~900 bytes.
    let big = |extra: &str| {
        let paths: serde_json::Map<String, Value> = (0..CAP_PATHS)
            .map(|i| (format!("/{extra}{i:03}{}", "x".repeat(900)), json!(1)))
            .collect();
        serde_json::to_vec(&json!({"v": 1, "views": 500, "paths": paths})).unwrap()
    };
    let days3 = ["2026-10-28", "2026-10-29", "2026-10-30"];
    let before: Vec<Vec<u8>> = days3.iter().map(|_| big("p")).collect();
    assert!(before[0].len() > 440_000 && before[0].len() < 500_000);
    for (d, blob) in days3.iter().zip(&before) {
        kv_set(&site, &format!("day:{d}"), blob).await;
    }
    feed(&site, days3.iter().map(|d| view(d, "/")).collect()).await;

    let after: Vec<Value> = {
        let mut v = Vec::new();
        for d in days3 {
            v.push(day(&site, d).await.unwrap());
        }
        v
    };
    assert_eq!(after[2]["views"], 501);
    assert_eq!(after[1]["views"], 501);
    // The oldest did not fit in the budget: unchanged.
    assert_eq!(after[0]["views"], 500);
    assert_eq!(kv_get(&site, "day:2026-10-28").await.unwrap(), before[0]);
    site
}

/// A blob that is not JSON is replaced (the day starts again); one with a version the
/// plugin does not know is left alone, and that day's events are not counted.
async fn stored_blobs_that_are_corrupt_or_newer_are_handled(site: Site) -> Site {
    let site = site_with_analytics(site).await;
    kv_set(&site, &format!("day:{D1}"), b"not json at all").await;
    let future = br#"{"v":2,"hits":99}"#;
    kv_set(&site, &format!("day:{D2}"), future).await;
    feed(&site, vec![view(D1, "/"), view(D2, "/")]).await;
    assert_eq!(day(&site, D1).await.unwrap()["views"], 1);
    assert_eq!(kv_get(&site, &format!("day:{D2}")).await.unwrap(), future);
    site
}

async fn a_malformed_payload_is_refused(site: Site) -> Site {
    let site = site_with_analytics(site).await;
    for payload in [
        json!({}),
        json!({"events": "x"}),
        json!([1, 2]),
        json!(null),
    ] {
        let err = site
            .host
            .dispatch_action(
                PLUGIN,
                "request.served",
                ActionEvent {
                    subject: None,
                    payload,
                },
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("invalid"), "{err}");
    }
    // An empty batch is fine and writes nothing.
    feed(&site, Vec::new()).await;
    assert!(days(&site).await.is_empty());
    assert_eq!(latest(&site).await, None);
    site
}
