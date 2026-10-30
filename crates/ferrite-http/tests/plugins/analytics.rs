//! The `analytics` example plugin through a real host (M4-3a Task 3, plan P44 and P46):
//! batches of `request.served` events become one KV blob per UTC day, the stats page
//! shows them to the listed viewers only, retention prunes back from today, the maps are
//! capped, and a hostile path or referrer is escaped (Review Focus 2). Ruling X7: days past
//! `detail_days` are compacted to their totals, and the prune and the compaction run before
//! counting, so a store at its quota recovers.

use std::sync::Arc;

use ferrite_http::{CacheOutcome, ServedEvent, StoreHostServices};
use ferrite_plugin_host::{
    ActionEvent, Grant, HostConfig, HostServices, KindOk, Manifest, PluginHost, Post, RouteRequest,
    RouteResponse, SettingSpec,
};
use ferrite_store::Store;
use serde_json::{Value, json};

use crate::common::{Site, plugin_manifest, plugin_wasm};
use crate::profile_tests;

profile_tests!(
    a_batch_is_counted_per_day,
    excluded_paths_follow_the_setting,
    stats_page_needs_a_listed_viewer,
    stats_page_escapes_and_truncates,
    stats_page_without_data,
    stats_page_reads_a_full_window,
    stats_page_reads_short_distinct_keys_over_ninety_days,
    the_newest_day_is_always_decoded_in_full,
    older_days_read_through_the_brief_path,
    a_full_batch_on_full_blobs_fits_the_call_limits,
    days_parameter_sums_the_window,
    retention_keeps_the_newest_days,
    a_future_event_cannot_push_history_out,
    retention_setting_is_clamped_and_defaults_to_ninety,
    prune_uses_the_rest_of_the_call_budget,
    maps_are_capped_with_other,
    a_batch_of_many_days_stays_within_the_call_budget,
    long_paths_and_hosts_are_stored_clipped,
    a_thousand_long_views_on_one_maximal_blob,
    a_midnight_batch_with_a_new_newest_day_and_a_maximal_older_one,
    a_thousand_views_that_only_increment_a_maximal_blob,
    a_batch_over_three_days_with_a_maximal_oldest,
    an_older_day_over_the_decode_budget_is_skipped,
    the_largest_blob_after_truncation_fits_the_stats_page,
    a_failed_read_never_overwrites_a_stored_day,
    a_thousand_hostile_paths_stay_inside_the_default_fuel,
    stored_blobs_that_are_corrupt_or_newer_are_handled,
    a_malformed_payload_is_ignored,
    an_old_day_is_compacted_to_its_totals,
    detail_days_follows_the_setting_and_a_backlog_clears,
    a_straggler_for_a_compacted_day_is_merged_into_it,
    the_prune_runs_before_counting,
    a_refused_write_still_prunes_and_the_next_batch_succeeds,
    compaction_makes_room_when_the_store_is_full,
    a_compaction_refused_for_another_reason_keeps_the_full_day,
    the_page_says_which_days_could_not_be_read,
    encoded_paths_are_cut_whole_and_shown_decoded,
    the_worst_batch_with_maintenance_fits_the_default_fuel,
);

const PLUGIN: &str = "analytics";
const CAP_PATHS: usize = 500;
const CAP_REFERRERS: usize = 200;

// ---- helpers --------------------------------------------------------------------------

/// A site with the plugin installed. `ANALYTICS_FUEL=<n>` installs it with a fuel limit of
/// `n` instead of the default, to bracket what a case needs (the figures in the task
/// report): `ANALYTICS_FUEL=25000000 cargo test -p ferrite-http --test plugins analytics`.
async fn site_with_analytics(site: Site) -> Site {
    let fuel = std::env::var("ANALYTICS_FUEL").ok();
    site_with_fuel(site, fuel.as_deref()).await
}

/// The same with a fuel limit of `fuel` (granted, as an approved install would be).
async fn site_with_fuel(site: Site, fuel: Option<&str>) -> Site {
    clear_of_midnight().await;
    let site = crate::with_generous_deadlines(site);
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
    site
}

/// Waits until just after UTC midnight when it is less than two minutes away, so that a
/// case, which counts back from "today" on every call, does not straddle the change of day.
async fn clear_of_midnight() {
    let now = chrono::Utc::now();
    let midnight = (now.date_naive() + chrono::Duration::days(1))
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc();
    let left = midnight - now;
    if left < chrono::Duration::minutes(2) {
        tokio::time::sleep((left + chrono::Duration::seconds(1)).to_std().unwrap()).await;
    }
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

/// The compacted days (`totals:` keys) without their prefix, ascending.
async fn totals_days(site: &Site) -> Vec<String> {
    site.store
        .read()
        .plugin_kv()
        .list_keys(PLUGIN, "totals:", 200)
        .await
        .unwrap()
        .into_iter()
        .map(|k| k.strip_prefix("totals:").unwrap().to_owned())
        .collect()
}

/// Every day with a blob, full or compacted, ascending.
async fn stored_days(site: &Site) -> Vec<String> {
    let mut all = days(site).await;
    all.extend(totals_days(site).await);
    all.sort();
    all.dedup();
    all
}

/// The decoded compacted blob of `day`.
async fn totals(site: &Site, day: &str) -> Option<Value> {
    kv_get(site, &format!("totals:{day}"))
        .await
        .map(|b| serde_json::from_slice(&b).unwrap())
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

/// The UTC day `n` days before today (`n` below 0: after), as `YYYY-MM-DD`. The plugin
/// counts back from the wall clock, so the tests do too. (`site_with_fuel` waits out a
/// UTC midnight that is less than two minutes away; a case that runs longer than that and
/// is started just before it could still fail.) Leaked: a test process is short-lived.
fn ago(n: i64) -> &'static str {
    let day = chrono::Utc::now().date_naive() - chrono::Duration::days(n);
    Box::leak(day.format("%Y-%m-%d").to_string().into_boxed_str())
}

/// Two days that are safely inside the window and not in the future.
fn d1() -> &'static str {
    ago(2)
}

fn d2() -> &'static str {
    ago(1)
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
            event(d1(), "08:00:00.000000", "/", 200, news, "browser"),
            event(d1(), "08:00:01.000000", "/a/", 200, None, "browser"),
            event(d1(), "08:00:02.000000", "/a/", 304, news, "browser"),
            event(d1(), "08:00:03.000000", "/missing/", 404, None, "browser"),
            event(d1(), "08:00:04.000000", "/", 200, None, "bot"),
            event(d1(), "08:00:05.000000", "/feed/", 200, None, "other"),
            event(
                d1(),
                "08:00:06.000000",
                "/x/",
                200,
                Some("example.com"),
                "browser",
            ),
            event(
                d1(),
                "08:00:07.000000",
                "/admin/users",
                200,
                None,
                "browser",
            ),
            event(d1(), "08:00:08.000000", "/_p/foo", 200, None, "browser"),
            event(d1(), "23:59:59.999999", "/go/", 301, None, "browser"),
            event(
                d2(),
                "00:00:00.000000",
                "/",
                200,
                Some("search.example.net"),
                "browser",
            ),
            event(d2(), "00:00:01.000000", "/boom/", 500, None, "browser"),
            event(d2(), "00:00:02.000000", "/", 200, None, "browser"),
            // Unusable, and skipped: a time that is not a time.
            json!({"path": "/", "status": 200, "ts": "garbage", "referrer_host": null,
                   "ua_class": "browser", "cache": "hit"}),
        ],
    )
    .await;

    assert_eq!(
        day(&site, d1()).await.unwrap(),
        json!({
            "v": 1, "views": 4, "bots": 1, "other": 1,
            "status": {"2xx": 5, "3xx": 2, "4xx": 1},
            "paths": {"/": 1, "/a/": 2, "/x/": 1},
            "referrers": {"news.example.org": 2},
        })
    );
    assert_eq!(
        day(&site, d2()).await.unwrap(),
        json!({
            "v": 1, "views": 2, "bots": 0, "other": 0,
            "status": {"2xx": 2, "5xx": 1},
            "paths": {"/": 2},
            "referrers": {"search.example.net": 1},
        })
    );
    assert_eq!(days(&site).await, [d1(), d2()]);

    // A second batch adds to the stored blobs.
    feed(
        &site,
        vec![
            event(d2(), "01:00:00.000000", "/", 200, news, "browser"),
            event(d2(), "01:00:01.000000", "/new/", 404, None, "browser"),
        ],
    )
    .await;
    let blob = day(&site, d2()).await.unwrap();
    assert_eq!(blob["views"], 3);
    assert_eq!(blob["status"], json!({"2xx": 3, "4xx": 1, "5xx": 1}));
    assert_eq!(blob["paths"], json!({"/": 3}));
    assert_eq!(
        blob["referrers"],
        json!({"news.example.org": 1, "search.example.net": 1})
    );
    assert_eq!(days(&site).await, [d1(), d2()]);
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
        .map(|p| view(d1(), p))
        .collect(),
    )
    .await;
    assert_eq!(
        day(&site, d1()).await.unwrap()["paths"],
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
        .map(|p| view(d2(), p))
        .collect(),
    )
    .await;
    assert_eq!(
        day(&site, d2()).await.unwrap()["paths"],
        json!({"/privately": 1, "/admin/z": 1, "/nope": 1})
    );

    // An empty setting excludes nothing.
    setting(&site, "exclude_paths", json!("")).await;
    feed(&site, vec![view(ago(0), "/admin/users")]).await;
    assert_eq!(
        day(&site, ago(0)).await.unwrap()["paths"],
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
    feed(&site, vec![view(d1(), "/")]).await;

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
            event(d1(), "10:00:00.000000", hostile, 200, Some(host), "browser"),
            event(
                d1(),
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
    // The plugin stores a path cut to 200 bytes and a host cut to 100, so these show whole.
    assert!(
        html.contains(&format!("<code>/{}</code>", "a".repeat(199))),
        "{html}"
    );
    assert!(
        html.contains(&format!("<code>{}</code>", "h".repeat(100))),
        "{html}"
    );
    assert!(!html.contains(&"a".repeat(200)), "{html}");
    assert!(!html.contains(&"h".repeat(101)), "{html}");
    // Totals, a chart without script, and the two tables.
    assert_eq!(stat(&html, "views"), 2);
    assert!(html.contains("<svg"), "{html}");

    // The page clips on its own too, for a blob it did not write: 200 characters, then an
    // ellipsis.
    let foreign = format!(
        r#"{{"v":1,"views":1,"paths":{{"/{}":1}},"referrers":{{"{}":1}}}}"#,
        "z".repeat(299),
        "q".repeat(300)
    );
    kv_set(&site, &format!("day:{}", d2()), foreign.as_bytes()).await;
    let (_, html) = stats(&site, &[]).await;
    assert!(html.contains(&format!("/{}…", "z".repeat(199))), "{html}");
    assert!(!html.contains(&"z".repeat(200)), "{html}");
    assert!(html.contains(&format!("{}…", "q".repeat(200))), "{html}");
    assert!(!html.contains(&"q".repeat(201)), "{html}");
    site
}

async fn stats_page_without_data(site: Site) -> Site {
    let site = viewer_site(site).await;
    let (status, html) = stats(&site, &[]).await;
    assert_eq!(status, 200);
    assert!(html.contains("No data yet for the last 30 days"), "{html}");
    assert!(!html.contains("data-stat"), "{html}");
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
    for d in [d1(), d2()] {
        kv_set(&site, &format!("day:{d}"), &full_blob("blog")).await;
    }
    let events: Vec<Value> = (0..1000)
        .map(|i| {
            let d = if i % 2 == 0 { d1() } else { d2() };
            let path = format!("/blog/a-reasonably-long-post-title-{:03}/", i % CAP_PATHS);
            let referrer = format!("referrer-{:03}.example.org", i % CAP_REFERRERS);
            event(d, "12:00:00.000000", &path, 200, Some(&referrer), "browser")
        })
        .collect();
    feed(&site, events).await;
    // Day 1 got the even events, day 2 the odd ones; each path of the stored blob that an
    // event named (twice) went from 3 to 5.
    for (d, hit) in [(d1(), "000"), (d2(), "001")] {
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

// ---- retention ------------------------------------------------------------------------

// ---- caps and budgets -----------------------------------------------------------------

/// P44: `paths` and `referrers` are capped at 500 and 200 keys; past that, new keys count
/// under "(other)". Keys already stored keep counting.
async fn maps_are_capped_with_other(site: Site) -> Site {
    let site = site_with_analytics(site).await;
    let events: Vec<Value> = (0..520)
        .map(|i| {
            let referrer = (i < 220).then(|| format!("h{i:03}.example.org"));
            event(
                d1(),
                "09:00:00.000000",
                &format!("/p{i:03}"),
                200,
                referrer.as_deref(),
                "browser",
            )
        })
        .collect();
    feed(&site, events).await;

    let blob = day(&site, d1()).await.unwrap();
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
                d1(),
                "09:01:00.000000",
                "/p000",
                200,
                Some("h000.example.org"),
                "browser",
            ),
            event(
                d1(),
                "09:01:01.000000",
                "/new",
                200,
                Some("new.example.org"),
                "browser",
            ),
        ],
    )
    .await;
    let blob = day(&site, d1()).await.unwrap();
    assert_eq!(blob["views"], 522);
    assert_eq!(blob["paths"]["/p000"], 2);
    assert_eq!(blob["paths"]["(other)"], 21);
    assert_eq!(blob["paths"].as_object().unwrap().len(), CAP_PATHS + 1);
    assert_eq!(blob["referrers"]["h000.example.org"], 2);
    assert_eq!(blob["referrers"]["(other)"], 21);
    assert!(
        kv_get(&site, &format!("day:{}", d1())).await.unwrap().len() < 1024 * 1024,
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

/// A blob that is not JSON is replaced (the day starts again); one with a version the
/// plugin does not know is left alone, and that day's events are not counted.
async fn stored_blobs_that_are_corrupt_or_newer_are_handled(site: Site) -> Site {
    let site = site_with_analytics(site).await;
    kv_set(&site, &format!("day:{}", d1()), b"not json at all").await;
    let future = br#"{"v":2,"hits":99}"#;
    kv_set(&site, &format!("day:{}", d2()), future).await;
    feed(&site, vec![view(d1(), "/"), view(d2(), "/")]).await;
    assert_eq!(day(&site, d1()).await.unwrap()["views"], 1);
    assert_eq!(
        kv_get(&site, &format!("day:{}", d2())).await.unwrap(),
        future
    );
    site
}

// ---- the clock, retention and the stats window ------------------------------------------

/// The plugin's fixed limits for decoding the stats page (see `FULL_DECODE_*` in its
/// `lib.rs`).
const FULL_DAYS: usize = 4;
const FULL_BYTES: usize = 150_000;

/// A stored blob with a view count only, enough for the prune and the window.
const STUB: &[u8] = br#"{"v":1,"views":5}"#;

async fn seed_days(site: &Site, ago_days: &[i64]) {
    for &n in ago_days {
        kv_set(site, &format!("day:{}", ago(n)), STUB).await;
    }
}

/// `?days=N` sums the N days ending today; an unusable N means 30 and a large one is
/// capped at 90.
async fn days_parameter_sums_the_window(site: Site) -> Site {
    let site = viewer_site(site).await;
    // Day i (0..40) is 39 - i days ago and has i + 1 views and one bot, one batch each.
    for i in 0..40i64 {
        let d = ago(39 - i);
        feed(
            &site,
            (0..=i)
                .map(|_| view(d, "/"))
                .chain([event(d, "13:00:00.000000", "/bot/", 200, None, "bot")])
                .collect(),
        )
        .await;
    }

    // Today (i = 39) has 40 views; the window of N days holds the N newest.
    let sum = |from: i64| (from..40).map(|i| i as u64 + 1).sum::<u64>();
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

/// "Keep N days" keeps the newest N counted back from today; an event older than that is
/// not stored, and one at most a day ahead of today is.
async fn retention_keeps_the_newest_days(site: Site) -> Site {
    let site = site_with_analytics(site).await;
    setting(&site, "retention_days", json!(3)).await;
    for n in 1..=5 {
        kv_set(&site, &format!("day:{}", ago(n)), STUB).await;
    }
    feed(&site, vec![view(ago(0), "/")]).await;
    assert_eq!(days(&site).await, [ago(2), ago(1), ago(0)]);
    // A straggler from before the window is dropped, not stored and pruned later.
    feed(&site, vec![view(ago(3), "/old/")]).await;
    assert_eq!(days(&site).await, [ago(2), ago(1), ago(0)]);

    // Retention 1: only today (and tomorrow, which a node a little ahead may report).
    setting(&site, "retention_days", json!(1)).await;
    feed(&site, vec![view(ago(0), "/")]).await;
    assert_eq!(days(&site).await, [ago(0)]);
    feed(
        &site,
        vec![view(ago(1), "/"), view(ago(0), "/"), view(ago(-1), "/")],
    )
    .await;
    assert_eq!(days(&site).await, [ago(0), ago(-1)]);
    // Two days ahead is too far.
    feed(&site, vec![view(ago(-2), "/")]).await;
    assert_eq!(days(&site).await, [ago(0), ago(-1)]);
    site
}

/// An event dated far ahead (a node whose clock is wrong) is ignored: it does not move
/// "today", so nothing is pruned and the events that follow are still counted.
async fn a_future_event_cannot_push_history_out(site: Site) -> Site {
    let site = site_with_analytics(site).await;
    for n in [80, 3, 2, 1] {
        kv_set(&site, &format!("day:{}", ago(n)), STUB).await;
    }
    // Only future events: nothing is counted or deleted (day 80 is past the detail window,
    // so it is compacted, its count kept), and the call is fine.
    feed(
        &site,
        vec![
            view(ago(-400), "/"),
            view(ago(-2), "/"),
            view(ago(-5000), "/"),
        ],
    )
    .await;
    assert_eq!(days(&site).await, [ago(3), ago(2), ago(1)]);
    assert_eq!(totals_days(&site).await, [ago(80)]);
    // With a real event in the same batch, only that one is counted.
    feed(&site, vec![view(ago(-400), "/"), view(ago(0), "/")]).await;
    assert_eq!(
        stored_days(&site).await,
        [ago(80), ago(3), ago(2), ago(1), ago(0)]
    );
    for n in [3, 2, 1] {
        assert_eq!(day(&site, ago(n)).await.unwrap()["views"], 5, "{}", ago(n));
    }
    assert_eq!(totals(&site, ago(80)).await.unwrap()["views"], 5);
    assert_eq!(day(&site, ago(0)).await.unwrap()["views"], 1);
    site
}

/// Valid is 1..=366: more is 366, less is 1, and an unset or unusable value is 90.
async fn retention_setting_is_clamped_and_defaults_to_ninety(site: Site) -> Site {
    let site = site_with_analytics(site).await;

    // Unset: the manifest default, 90 days, so the 89th day back is the oldest kept (and
    // compacted: it is past the 30 days of detail).
    seed_days(&site, &[90, 89]).await;
    feed(&site, vec![view(ago(0), "/")]).await;
    assert_eq!(stored_days(&site).await, [ago(89), ago(0)]);

    // Not a number: 90 again.
    setting(&site, "retention_days", json!("many")).await;
    seed_days(&site, &[90]).await;
    feed(&site, vec![view(ago(0), "/")]).await;
    assert_eq!(stored_days(&site).await, [ago(89), ago(0)]);

    // 5000 is 366: the 365th day back is kept, the 366th goes.
    setting(&site, "retention_days", json!(5000)).await;
    seed_days(&site, &[366, 365]).await;
    feed(&site, vec![view(ago(0), "/")]).await;
    assert_eq!(stored_days(&site).await, [ago(365), ago(89), ago(0)]);
    assert_eq!(totals_days(&site).await, [ago(365), ago(89)]);

    // 0 and negative are 1: only today stays, full and compacted days alike.
    for v in [json!(0), json!(-7)] {
        setting(&site, "retention_days", v).await;
        seed_days(&site, &[89, 1]).await;
        feed(&site, vec![view(ago(0), "/")]).await;
        assert_eq!(stored_days(&site).await, [ago(0)]);
    }
    site
}

/// The prune deletes with what the call's 100 operations leave, and the next batches
/// finish the job.
async fn prune_uses_the_rest_of_the_call_budget(site: Site) -> Site {
    let site = site_with_analytics(site).await;
    setting(&site, "retention_days", json!(5)).await;
    // 130 days long expired, oldest first.
    let old: Vec<&str> = (0..130).map(|i| ago(1129 - i)).collect();
    for d in &old {
        kv_set(&site, &format!("day:{d}"), br#"{"v":1}"#).await;
    }
    // Of the 100 operations: the listing of the full days, then deletes while more than 10
    // are left (room for one compaction, 6, the listing of the compacted days, 1, and
    // counting one day, 3: its blob in, the check that a missing blob is really missing,
    // the blob out): 89 deletes. The listing and the counting take 4 of the 10.
    feed(&site, vec![view(ago(0), "/")]).await;
    let left = days(&site).await;
    assert_eq!(left.len(), 130 - 89 + 1, "{left:?}");
    assert!(left.contains(&ago(0).to_owned()));
    // The oldest go first.
    assert_eq!(left[0], old[89]);

    feed(&site, vec![view(ago(0), "/")]).await;
    assert_eq!(days(&site).await, [ago(0)]);
    assert_eq!(day(&site, ago(0)).await.unwrap()["views"], 2);
    site
}

/// Ten days in one batch (a clock that stepped): the newest three are counted and the call
/// stays within its 100 KV operations, so it does not fail.
async fn a_batch_of_many_days_stays_within_the_call_budget(site: Site) -> Site {
    let site = site_with_analytics(site).await;
    let events: Vec<Value> = (1..=10).rev().map(|n| view(ago(n), "/")).collect();
    feed(&site, events).await;
    let want: Vec<&str> = (1..=3).rev().map(ago).collect();
    assert_eq!(days(&site).await, want);
    site
}

/// Core always sends `{"events":[…]}` of events of one shape; anything else is logged and
/// dropped whole rather than failing the call (a retry cannot fix it, and failures count towards auto-disable).
async fn a_malformed_payload_is_ignored(site: Site) -> Site {
    let site = site_with_analytics(site).await;
    for payload in [
        json!({}),
        json!({"events": "x"}),
        // One event of another shape drops the batch: core never sends one.
        json!({"events": [view(d1(), "/"), {"path": 7}]}),
        json!({"events": [view(d1(), "/"), 7]}),
        json!([1, 2]),
        json!(null),
    ] {
        site.host
            .dispatch_action(
                PLUGIN,
                "request.served",
                ActionEvent {
                    subject: None,
                    payload,
                },
            )
            .await
            .unwrap();
    }
    // An empty batch is fine and writes nothing.
    feed(&site, Vec::new()).await;
    assert!(days(&site).await.is_empty());
    assert!(site.host.is_enabled(PLUGIN));
    site
}

// ---- the stats page decode budget -------------------------------------------------------

/// A saturated blob (500 paths, 200 referrers) in the layout the plugin writes, with the
/// keys from `path` and `referrer`.
fn saturated(path: &dyn Fn(usize) -> String, referrer: &dyn Fn(usize) -> String) -> Vec<u8> {
    let map = |n: usize, key: &dyn Fn(usize) -> String| {
        Value::Object((0..n).map(|i| (key(i), json!(3))).collect())
    };
    format!(
        r#"{{"v":1,"views":1500,"bots":10,"other":5,"status":{{"2xx":1600,"4xx":7}},"paths":{},"referrers":{}}}"#,
        map(CAP_PATHS, path),
        map(CAP_REFERRERS, referrer),
    )
    .into_bytes()
}

/// The 90 days a page can show, each with 500 long paths and 200 referrers (about 33 KB):
/// the totals and the chart cover every day, the lists the newest days that fit the decode
/// budget, and the call stays within the default fuel and its 100 KV operations.
async fn stats_page_reads_a_full_window(site: Site) -> Site {
    let site = viewer_site(site).await;
    let blob = saturated(
        &|i| format!("/blog/2026/a-reasonably-long-post-title-{i:03}/"),
        &|i| format!("referrer-{i:03}.example.org"),
    );
    assert!(blob.len() > 30_000, "{}", blob.len());
    for n in 0..90 {
        kv_set(&site, &format!("day:{}", ago(n)), &blob).await;
    }

    let (status, html) = stats(&site, &[("days", "90")]).await;
    assert_eq!(status, 200, "{}", &html[..html.len().min(300)]);
    assert_eq!(stat(&html, "views"), 90 * 1500);
    assert_eq!(stat(&html, "bots"), 90 * 10);
    assert_eq!(stat(&html, "status-4xx"), 90 * 7);
    assert_eq!(html.matches("class=\"bar\"").count(), 90);
    let listed = (FULL_BYTES / blob.len()).clamp(1, FULL_DAYS);
    assert!(
        html.contains(&format!(
            "cover the newest {listed} of the 90 days with data"
        )),
        "{html}"
    );
    assert!(html.contains("referrer-000.example.org"));
    assert!(html.contains("/blog/2026/a-reasonably-long-post-title-000/"));

    // A shorter period that fits entirely has no note.
    let (status, html) = stats(&site, &[("days", "3")]).await;
    assert_eq!(status, 200);
    assert!(!html.contains("data-note"), "{html}");
    site
}

/// Short keys that differ from day to day (500 paths and 200 referrers each, so the page
/// sums up to `FULL_DAYS` x 700 different keys): the entry count, not the byte count, is
/// the cost. The call stays under the default fuel (`ANALYTICS_FUEL` brackets it).
async fn stats_page_reads_short_distinct_keys_over_ninety_days(site: Site) -> Site {
    let site = viewer_site(site).await;
    for n in 0..90usize {
        let blob = saturated(&|i| format!("/{n:02}{i:03}"), &|i| {
            format!("{n:02}{i:03}.io")
        });
        kv_set(&site, &format!("day:{}", ago(n as i64)), &blob).await;
    }
    let (status, html) = stats(&site, &[("days", "90")]).await;
    assert_eq!(status, 200, "{}", &html[..html.len().min(300)]);
    assert_eq!(stat(&html, "views"), 90 * 1500);
    assert!(
        html.contains(&format!(
            "cover the newest {FULL_DAYS} of the 90 days with data"
        )),
        "{html}"
    );
    // The newest day's keys are listed; the oldest day's are not.
    assert!(html.contains("/00000"), "{html}");
    assert!(!html.contains("/89000"), "{html}");
    site
}

/// The newest day with data is always decoded with its lists, however big its blob. The
/// plugin now writes at most about 130 KB a day, so this one is a blob from before the
/// path cut (or a foreign one): 500 paths of 350 bytes and 200 hosts of 253, over 200 KB,
/// which is over the byte budget on its own.
async fn the_newest_day_is_always_decoded_in_full(site: Site) -> Site {
    let site = viewer_site(site).await;
    let big = saturated(&|i| format!("/{}{i:03}", "%D0%B0".repeat(58)), &|i| {
        format!("{i:03}{}.org", "h".repeat(247))
    });
    assert!(big.len() > FULL_BYTES, "{}", big.len());
    kv_set(&site, &format!("day:{}", ago(0)), &big).await;
    kv_set(&site, &format!("day:{}", ago(1)), STUB).await;

    let (status, html) = stats(&site, &[("days", "1")]).await;
    assert_eq!(status, 200);
    assert!(html.contains("000hhhh"), "{html}");
    assert!(!html.contains("data-note"), "{html}");

    // The older day is not decoded in full once the newest took the budget.
    let (_, html) = stats(&site, &[("days", "2")]).await;
    assert!(
        html.contains("cover the newest 1 of the 2 days with data"),
        "{html}"
    );
    assert_eq!(stat(&html, "views"), 1500 + 5);
    site
}

/// Days the plugin wrote itself, read through the brief path (the older ones, past the
/// day budget): their totals and status classes must come out right, which depends on
/// the counts being written ahead of the maps.
async fn older_days_read_through_the_brief_path(site: Site) -> Site {
    let site = viewer_site(site).await;
    // Eight days, oldest first: one view, one bot and `k + 1` 404s each.
    for k in 0..8i64 {
        let d = ago(7 - k);
        let mut events = vec![
            view(d, "/"),
            event(d, "10:00:00.000000", "/", 200, None, "bot"),
        ];
        events.extend((0..=k).map(|i| {
            event(
                d,
                "11:00:00.000000",
                &format!("/gone/{i}/"),
                404,
                None,
                "browser",
            )
        }));
        feed(&site, events).await;
        let blob = kv_get(&site, &format!("day:{d}")).await.unwrap();
        assert!(
            blob.starts_with(br#"{"v":1,"views":"#),
            "counts first: {}",
            String::from_utf8_lossy(&blob)
        );
    }
    let (status, html) = stats(&site, &[("days", "8")]).await;
    assert_eq!(status, 200);
    assert_eq!(stat(&html, "views"), 8);
    assert_eq!(stat(&html, "bots"), 8);
    assert_eq!(stat(&html, "status-2xx"), 16);
    assert_eq!(stat(&html, "status-4xx"), 36);
    let listed = FULL_DAYS;
    assert!(
        html.contains(&format!(
            "cover the newest {listed} of the 8 days with data"
        )),
        "{html}"
    );
    site
}

// ---- long keys, the batch decode budget and a failed read --------------------------------

/// A blob as large as the plugin can write: 500 paths of 200 bytes and 200 hosts of 100,
/// about 130 KB, in the layout the plugin writes.
fn maximal_blob() -> Vec<u8> {
    saturated(&|i| format!("/{i:03}{}", "p".repeat(196)), &|i| {
        format!("{i:03}{}", "h".repeat(97))
    })
}

/// A browser view the way core can report the worst of them: a 508-byte percent-encoded
/// path (84 Cyrillic letters) that differs from the next within its first 200 bytes, and a
/// 253-byte referrer host.
fn long_view(day: &str, i: usize) -> Value {
    let path = format!("/{i:03}{}", "%D0%B0".repeat(84));
    let host = format!("{i:03}{}", "h".repeat(250));
    assert_eq!((path.len(), host.len()), (508, 253));
    event(day, "12:00:00.000000", &path, 200, Some(&host), "browser")
}

/// Paths are stored cut to 200 bytes and referrer hosts to 100, so the length of a key an
/// attacker can choose is bounded; paths that differ only after the cut share one count.
async fn long_paths_and_hosts_are_stored_clipped(site: Site) -> Site {
    let site = site_with_analytics(site).await;
    let long = format!("/{}", "a".repeat(507));
    let other = format!("/{}b", "a".repeat(250));
    // 150 two-byte characters: byte 200 falls inside one, so the cut is at 199.
    let wide = format!("/{}", "é".repeat(150));
    feed(
        &site,
        vec![
            event(
                d1(),
                "09:00:00.000000",
                &long,
                200,
                Some(&"h".repeat(253)),
                "browser",
            ),
            event(d1(), "09:00:01.000000", &other, 200, None, "browser"),
            event(d1(), "09:00:02.000000", &wide, 200, None, "browser"),
            // The site's own host is not counted as a referrer.
            event(
                d1(),
                "09:00:03.000000",
                "/",
                200,
                Some("example.com"),
                "browser",
            ),
        ],
    )
    .await;
    let blob = day(&site, d1()).await.unwrap();
    assert_eq!(blob["views"], 4);
    let paths = blob["paths"].as_object().unwrap();
    assert_eq!(
        paths[&long[..200]],
        2,
        "the first two share their first 200 bytes"
    );
    assert_eq!(paths[&wide[..199]], 1);
    assert_eq!(paths["/"], 1);
    assert_eq!(paths.len(), 3);
    let mut hosts = serde_json::Map::new();
    hosts.insert("h".repeat(100), json!(1));
    assert_eq!(blob["referrers"], Value::Object(hosts));
    site
}

/// 1000 views with 508-byte paths and 253-byte hosts on a maximal stored day: counted
/// inside the default fuel (`ANALYTICS_FUEL` brackets it).
async fn a_thousand_long_views_on_one_maximal_blob(site: Site) -> Site {
    let site = site_with_analytics(site).await;
    kv_set(&site, &format!("day:{}", d1()), &maximal_blob()).await;
    feed(&site, (0..1000).map(|i| long_view(d1(), i)).collect()).await;
    let blob = day(&site, d1()).await.unwrap();
    assert_eq!(blob["views"], 1500 + 1000);
    let paths = blob["paths"].as_object().unwrap();
    assert_eq!(paths.len(), CAP_PATHS + 1);
    assert!(paths.keys().all(|k| k.len() <= 200), "paths are cut");
    assert!(
        blob["referrers"]
            .as_object()
            .unwrap()
            .keys()
            .all(|k| k.len() <= 100),
        "hosts are cut"
    );
    site
}

/// A batch across midnight on two maximal stored days: the newest day is counted, and the
/// older one, which would take the batch past its decode budget, is skipped (logged, its
/// events lost, the blob untouched) instead of the call trapping on fuel. At the default
/// fuel.
async fn an_older_day_over_the_decode_budget_is_skipped(site: Site) -> Site {
    let site = site_with_analytics(site).await;
    let before = maximal_blob();
    assert!(
        before.len() > 100_000 && before.len() < 150_000,
        "{}",
        before.len()
    );
    for d in [d1(), d2()] {
        kv_set(&site, &format!("day:{d}"), &before).await;
    }
    let events = (0..1000)
        .map(|i| long_view(if i % 2 == 0 { d1() } else { d2() }, i))
        .collect();
    feed(&site, events).await;
    assert_eq!(day(&site, d2()).await.unwrap()["views"], 1500 + 500);
    assert_eq!(day(&site, d1()).await.unwrap()["views"], 1500);
    assert_eq!(
        kv_get(&site, &format!("day:{}", d1())).await.unwrap(),
        before
    );

    // Small days do fit together: both are counted.
    feed(
        &site,
        vec![view(ago(4), "/"), view(ago(3), "/"), view(ago(5), "/")],
    )
    .await;
    assert_eq!(days(&site).await, [ago(5), ago(4), ago(3), d1(), d2()]);
    site
}

/// The page over the largest blob the plugin writes: `?days=1`, and 90 days of them (the
/// newest in full, the rest by their totals), both at the default fuel.
async fn the_largest_blob_after_truncation_fits_the_stats_page(site: Site) -> Site {
    let site = viewer_site(site).await;
    let blob = maximal_blob();
    for n in 0..90 {
        kv_set(&site, &format!("day:{}", ago(n)), &blob).await;
    }
    let (status, html) = stats(&site, &[("days", "1")]).await;
    assert_eq!(status, 200);
    assert_eq!(stat(&html, "views"), 1500);
    assert!(html.contains("000hhhh"), "{html}");
    assert!(!html.contains("data-note"), "{html}");

    let (status, html) = stats(&site, &[("days", "90")]).await;
    assert_eq!(status, 200);
    assert_eq!(stat(&html, "views"), 90 * 1500);
    assert!(
        html.contains("cover the newest 1 of the 90 days with data"),
        "{html}"
    );
    site
}

/// [`StoreHostServices`] with faults: `kv_get` of the keys in `failing_get` answers `None`,
/// as the host does for a store read that failed (it cannot tell that from a missing key),
/// and with `quota` set, `kv_set` refuses a write that would take the plugin's stored bytes
/// past that many, as the host does at its 10 MiB (the same store call, a smaller limit).
/// Writes of a key starting with one of `failing_set` answer the host's "storage error".
/// Also used by the `contact-form` cases.
pub(crate) struct Flaky {
    pub(crate) inner: Arc<StoreHostServices>,
    pub(crate) failing_get: Vec<String>,
    pub(crate) failing_set: Vec<String>,
    pub(crate) quota: Option<(Arc<dyn Store>, u64)>,
}

impl Flaky {
    /// Reads of `key` fail.
    pub(crate) fn failing_get(inner: Arc<StoreHostServices>, key: &str) -> Self {
        Self {
            inner,
            failing_get: vec![key.to_owned()],
            failing_set: Vec::new(),
            quota: None,
        }
    }

    /// Writes past `bytes` in all are refused.
    fn quota(site: &Site, bytes: u64) -> Self {
        Self {
            inner: site.services.clone(),
            failing_get: Vec::new(),
            failing_set: Vec::new(),
            quota: Some((site.store.clone(), bytes)),
        }
    }
}

#[async_trait::async_trait]
impl HostServices for Flaky {
    async fn get_post(&self, id: &str, kind_ok: &KindOk) -> Option<Post> {
        self.inner.get_post(id, kind_ok).await
    }
    async fn post_kind(&self, id: &str) -> Option<String> {
        self.inner.post_kind(id).await
    }
    async fn get_post_by_slug(&self, kind: &str, slug: &str) -> Option<Post> {
        self.inner.get_post_by_slug(kind, slug).await
    }
    async fn list_posts(&self, kind: &str, limit: u32, offset: u32) -> Vec<Post> {
        self.inner.list_posts(kind, limit, offset).await
    }
    async fn meta_get(&self, post: &str, key: &str) -> Option<String> {
        self.inner.meta_get(post, key).await
    }
    async fn meta_set(&self, post: &str, key: &str, value: &str) -> Result<(), String> {
        self.inner.meta_set(post, key, value).await
    }
    async fn option_get(&self, key: &str) -> Option<String> {
        self.inner.option_get(key).await
    }
    async fn kv_get(&self, plugin: &str, key: &str) -> Option<Vec<u8>> {
        if self.failing_get.iter().any(|k| k == key) {
            return None;
        }
        self.inner.kv_get(plugin, key).await
    }
    async fn kv_set(&self, plugin: &str, key: &str, value: Vec<u8>) -> Result<(), String> {
        if self.failing_set.iter().any(|k| key.starts_with(k.as_str())) {
            return Err("storage error".into());
        }
        let Some((store, bytes)) = &self.quota else {
            return self.inner.kv_set(plugin, key, value).await;
        };
        match store
            .read()
            .plugin_kv()
            .set_with_quota(plugin, key, &value, *bytes, 10_000)
            .await
        {
            Ok(true) => Ok(()),
            Ok(false) => Err(format!("kv quota exceeded ({bytes} bytes, 10000 keys)")),
            Err(_) => Err("storage error".into()),
        }
    }
    async fn kv_delete(&self, plugin: &str, key: &str) {
        self.inner.kv_delete(plugin, key).await;
    }
    async fn kv_list(&self, plugin: &str, prefix: &str, limit: u32) -> Vec<String> {
        self.inner.kv_list(plugin, prefix, limit).await
    }
    async fn setting_get(&self, plugin: &str, key: &str, spec: &SettingSpec) -> Option<String> {
        self.inner.setting_get(plugin, key, spec).await
    }
    async fn notify_admins(&self, plugin: &str, subject: &str, text: &str) -> Result<(), String> {
        self.inner.notify_admins(plugin, subject, text).await
    }
}

/// A host of its own over `services`, with the plugin installed (default limits).
fn host_over(services: Flaky) -> PluginHost {
    let host = PluginHost::new(HostConfig::default(), Arc::new(services)).unwrap();
    let manifest = Manifest::parse(&plugin_manifest(PLUGIN)).unwrap();
    host.install(manifest, &plugin_wasm(PLUGIN)).unwrap();
    host
}

/// When the read of a day answers `None` but the key is there (the read failed), the day
/// is left alone: writing a fresh blob would replace a whole day's counts with one batch.
async fn a_failed_read_never_overwrites_a_stored_day(site: Site) -> Site {
    let site = site_with_analytics(site).await;
    let stored = br#"{"v":1,"views":5}"#;
    kv_set(&site, &format!("day:{}", d1()), stored).await;

    let host = host_over(Flaky::failing_get(
        site.services.clone(),
        &format!("day:{}", d1()),
    ));
    host.dispatch_action(
        PLUGIN,
        "request.served",
        payload(vec![view(d1(), "/"), view(d2(), "/")]),
    )
    .await
    .unwrap();

    // The unreadable day is untouched; the other, really missing, day is counted.
    assert_eq!(
        kv_get(&site, &format!("day:{}", d1())).await.unwrap(),
        stored
    );
    assert_eq!(day(&site, d2()).await.unwrap()["views"], 1);

    // With the read working, the same day is counted onto what is stored.
    feed(&site, vec![view(d1(), "/")]).await;
    assert_eq!(day(&site, d1()).await.unwrap()["views"], 6);
    site
}

/// An event built the way core builds it: `ServedEvent::new` over the raw request path
/// (which may hold a raw `\` or `"`), serialised as the job payload carries it.
fn core_event(raw_path: &str, status: u16) -> Value {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        axum::http::header::USER_AGENT,
        "Mozilla/5.0".parse().unwrap(),
    );
    serde_json::to_value(ServedEvent::new(
        raw_path,
        &headers,
        status,
        CacheOutcome::Miss,
    ))
    .unwrap()
}

/// 1000 requests, answered 404, for `/` followed by 507 backslashes or quotes, and 1000
/// answered 200 for such paths that differ early: core percent-encodes the path (X5), so
/// the payload holds no JSON escapes and the batch costs what any long path costs. At the
/// default fuel; before the encoding, the 404s alone needed 80 to 90 M and trapped.
async fn a_thousand_hostile_paths_stay_inside_the_default_fuel(site: Site) -> Site {
    let site = site_with_analytics(site).await;
    for ch in ['\\', '"'] {
        let tail = ch.to_string().repeat(503);
        let gone: Vec<Value> = (0..1000)
            .map(|_| core_event(&format!("/{}", ch.to_string().repeat(507)), 404))
            .collect();
        assert!(
            !gone[0].to_string().contains(&format!("\\{ch}")),
            "core's event holds no escaped {ch}: {}",
            gone[0]
        );
        feed(&site, gone).await;
        let views: Vec<Value> = (0..1000)
            .map(|i| core_event(&format!("/{i:03}{tail}"), 200))
            .collect();
        feed(&site, views).await;
    }
    let blob = day(&site, ago(0)).await.unwrap();
    assert_eq!(blob["status"], json!({"2xx": 2000, "4xx": 2000}));
    assert_eq!(blob["views"], 2000);
    let paths = blob["paths"].as_object().unwrap();
    assert!(
        paths
            .keys()
            .all(|k| k.len() <= 200 && !k.contains(['\\', '"']))
    );
    site
}

/// The long-path views of `long_view` at paths that, once cut, are exactly the keys of
/// `maximal_blob`: they only add to existing counts.
fn repeat_view(day: &str, i: usize) -> Value {
    let k = i % CAP_PATHS;
    let path = format!("/{k:03}{}", "p".repeat(504));
    let host = format!("{:03}{}", k % CAP_REFERRERS, "h".repeat(250));
    event(day, "12:00:00.000000", &path, 200, Some(&host), "browser")
}

/// Midnight: 500 long views for a day that has no blob yet (the newest) and 500 for the
/// day before, which holds a maximal blob. Both days are counted (only blobs that exist
/// count against the decode budget). The most this batch costs, at the default fuel.
async fn a_midnight_batch_with_a_new_newest_day_and_a_maximal_older_one(site: Site) -> Site {
    let site = site_with_analytics(site).await;
    kv_set(&site, &format!("day:{}", d1()), &maximal_blob()).await;
    let events = (0..1000)
        .map(|i| long_view(if i % 2 == 0 { d1() } else { d2() }, i))
        .collect();
    feed(&site, events).await;
    assert_eq!(day(&site, d1()).await.unwrap()["views"], 1500 + 500);
    assert_eq!(day(&site, d2()).await.unwrap()["views"], 500);
    site
}

/// 1000 views that only increment keys of a maximal blob (their cut paths and hosts are its
/// keys): no new key, the blob keeps its size, the cost is hashing and decoding it.
async fn a_thousand_views_that_only_increment_a_maximal_blob(site: Site) -> Site {
    let site = site_with_analytics(site).await;
    kv_set(&site, &format!("day:{}", d1()), &maximal_blob()).await;
    feed(&site, (0..1000).map(|i| repeat_view(d1(), i)).collect()).await;
    let blob = day(&site, d1()).await.unwrap();
    assert_eq!(blob["views"], 1500 + 1000);
    let paths = blob["paths"].as_object().unwrap();
    assert_eq!(paths.len(), CAP_PATHS, "nothing went to (other)");
    assert_eq!(paths[&format!("/000{}", "p".repeat(196))], 5);
    site
}

/// Three days in one batch, the third (oldest) with a maximal blob: the most days a batch
/// writes, 1000 long views spread over them, at the default fuel. Older days than three
/// are dropped.
async fn a_batch_over_three_days_with_a_maximal_oldest(site: Site) -> Site {
    let site = site_with_analytics(site).await;
    kv_set(&site, &format!("day:{}", ago(3)), &maximal_blob()).await;
    let events = (0..1000)
        .map(|i| long_view(ago([1, 2, 3, 4][i % 4]), i))
        .collect();
    feed(&site, events).await;
    assert_eq!(day(&site, ago(3)).await.unwrap()["views"], 1500 + 250);
    assert_eq!(day(&site, ago(2)).await.unwrap()["views"], 250);
    assert_eq!(day(&site, ago(1)).await.unwrap()["views"], 250);
    assert!(
        day(&site, ago(4)).await.is_none(),
        "a fourth day is dropped"
    );
    site
}

// ---- X7: compaction, prune before count, the quota --------------------------------------

/// The plugin's value bytes in the store.
async fn usage(site: &Site) -> u64 {
    site.store.read().plugin_kv().usage(PLUGIN).await.unwrap().0
}

/// A day past `detail_days` (default 30) is compacted: its counts move to `totals:<day>`
/// (no `paths`, no `referrers`), the full blob goes. The last day of the window keeps its
/// lists. The page sums compacted days into the totals and the chart, lists none of them,
/// and says why.
async fn an_old_day_is_compacted_to_its_totals(site: Site) -> Site {
    let site = viewer_site(site).await;
    for n in [29, 30, 45] {
        kv_set(&site, &format!("day:{}", ago(n)), &maximal_blob()).await;
    }
    feed(&site, vec![view(ago(0), "/")]).await;
    assert_eq!(days(&site).await, [ago(29), ago(0)]);
    assert_eq!(totals_days(&site).await, [ago(45), ago(30)]);
    for n in [30, 45] {
        let blob = kv_get(&site, &format!("totals:{}", ago(n))).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&blob).unwrap(),
            json!({"v": 1, "views": 1500, "bots": 10, "other": 5,
                   "status": {"2xx": 1600, "4xx": 7}}),
            "{}",
            ago(n)
        );
        assert!(blob.len() < 256, "{}", blob.len());
    }
    assert_eq!(
        day(&site, ago(29)).await.unwrap()["paths"]
            .as_object()
            .unwrap()
            .len(),
        500
    );

    let (status, html) = stats(&site, &[("days", "90")]).await;
    assert_eq!(status, 200);
    assert_eq!(stat(&html, "views"), 3 * 1500 + 1);
    assert_eq!(stat(&html, "status-4xx"), 3 * 7);
    assert!(
        html.contains("cover the newest 2 of the 4 days with data."),
        "{html}"
    );
    assert!(
        html.contains("2 older days keep only their totals"),
        "{html}"
    );
    assert!(!html.contains("Reading more would exceed"), "{html}");
    site
}

/// `detail_days` is clamped to 1..=60 (30 when unset or not a number) and never exceeds
/// `retention_days`. A backlog (a lowered setting) is compacted at most eight days a
/// batch, oldest first.
async fn detail_days_follows_the_setting_and_a_backlog_clears(site: Site) -> Site {
    let site = site_with_analytics(site).await;
    seed_days(&site, &(1..=12).collect::<Vec<_>>()).await;
    // 2 days of detail: days 2 to 12 (eleven) are due; the oldest eight go in one batch.
    setting(&site, "detail_days", json!(2)).await;
    feed(&site, vec![view(ago(0), "/")]).await;
    let want: Vec<&str> = (5..=12).rev().map(ago).collect();
    assert_eq!(totals_days(&site).await, want);
    assert_eq!(days(&site).await, [ago(4), ago(3), ago(2), ago(1), ago(0)]);
    feed(&site, vec![view(ago(0), "/")]).await;
    assert_eq!(days(&site).await, [ago(1), ago(0)]);
    assert_eq!(totals_days(&site).await.len(), 11);

    // 0 is 1: only today keeps its lists.
    setting(&site, "detail_days", json!(0)).await;
    feed(&site, vec![view(ago(0), "/")]).await;
    assert_eq!(days(&site).await, [ago(0)]);

    // 500 is 60: day 59 keeps its lists, days 61 and 70 are compacted.
    seed_days(&site, &[70, 61, 59, 4]).await;
    setting(&site, "detail_days", json!(500)).await;
    feed(&site, vec![view(ago(0), "/")]).await;
    assert_eq!(days(&site).await, [ago(59), ago(4), ago(0)]);
    assert!(totals_days(&site).await.contains(&ago(70).to_owned()));
    // But never more than the retention: with 5 days kept, everything older goes,
    // compacted or not, and the 5 days keep their lists.
    setting(&site, "retention_days", json!(5)).await;
    feed(&site, vec![view(ago(0), "/")]).await;
    assert_eq!(days(&site).await, [ago(4), ago(0)]);
    assert_eq!(totals_days(&site).await, [ago(4), ago(3), ago(2), ago(1)]);

    // Not a number: 30 again.
    setting(&site, "retention_days", json!(90)).await;
    setting(&site, "detail_days", json!("lots")).await;
    seed_days(&site, &[31, 29]).await;
    feed(&site, vec![view(ago(0), "/")]).await;
    assert!(totals_days(&site).await.contains(&ago(31).to_owned()));
    assert!(days(&site).await.contains(&ago(29).to_owned()));
    site
}

/// An event for a day already compacted (a node whose clock is behind, a short detail
/// window at midnight) is counted into a full blob of that day, and the next batch adds
/// its counts to the compacted ones.
async fn a_straggler_for_a_compacted_day_is_merged_into_it(site: Site) -> Site {
    let site = site_with_analytics(site).await;
    setting(&site, "detail_days", json!(1)).await;
    seed_days(&site, &[2]).await;
    feed(&site, vec![view(ago(0), "/")]).await;
    assert_eq!(totals(&site, ago(2)).await.unwrap()["views"], 5);
    // The straggler: counted into `day:` (after this batch's compaction).
    feed(&site, vec![view(ago(2), "/late/"), view(ago(0), "/")]).await;
    assert_eq!(day(&site, ago(2)).await.unwrap()["views"], 1);
    // The next batch merges it.
    feed(&site, vec![view(ago(0), "/")]).await;
    assert!(day(&site, ago(2)).await.is_none());
    assert_eq!(totals(&site, ago(2)).await.unwrap()["views"], 6);
    site
}

/// The prune runs before counting: a store whose quota is full of expired days takes the
/// batch's write, because the expired days were deleted first.
async fn the_prune_runs_before_counting(site: Site) -> Site {
    let site = site_with_analytics(site).await;
    setting(&site, "retention_days", json!(10)).await;
    for n in [20, 21, 22] {
        kv_set(&site, &format!("day:{}", ago(n)), &maximal_blob()).await;
    }
    kv_set(&site, &format!("day:{}", ago(1)), STUB).await;
    // Room for nothing new.
    let host = host_over(Flaky::quota(&site, usage(&site).await));
    host.dispatch_action(PLUGIN, "request.served", payload(vec![view(ago(0), "/")]))
        .await
        .unwrap();
    assert_eq!(days(&site).await, [ago(1), ago(0)]);
    assert_eq!(day(&site, ago(0)).await.unwrap()["views"], 1);
    site
}

/// A write refused on the quota fails the batch (the job is retried), but the prune has
/// run: the expired day is gone. Lowering the retention frees enough for the next batch.
async fn a_refused_write_still_prunes_and_the_next_batch_succeeds(site: Site) -> Site {
    let site = site_with_analytics(site).await;
    setting(&site, "retention_days", json!(10)).await;
    kv_set(&site, &format!("day:{}", ago(15)), STUB).await;
    for n in 1..=3 {
        kv_set(&site, &format!("day:{}", ago(n)), &maximal_blob()).await;
    }
    let host = host_over(Flaky::quota(&site, usage(&site).await));
    let err = host
        .dispatch_action(PLUGIN, "request.served", payload(vec![view(ago(0), "/")]))
        .await
        .unwrap_err();
    assert!(format!("{err:?}").contains("quota"), "{err:?}");
    assert_eq!(
        days(&site).await,
        [ago(3), ago(2), ago(1)],
        "pruned, not counted"
    );

    // Two days kept: days 2 and 3 go before the count, which then fits.
    setting(&site, "retention_days", json!(2)).await;
    host.dispatch_action(PLUGIN, "request.served", payload(vec![view(ago(0), "/")]))
        .await
        .unwrap();
    assert_eq!(days(&site).await, [ago(1), ago(0)]);
    assert_eq!(day(&site, ago(0)).await.unwrap()["views"], 1);
    assert!(host.is_enabled(PLUGIN));
    site
}

/// With the store exactly full, a compaction's small write is refused at first: the
/// full blob is deleted to make room and the totals written again, so the day keeps its
/// counts and the batch's own write fits after it.
async fn compaction_makes_room_when_the_store_is_full(site: Site) -> Site {
    let site = site_with_analytics(site).await;
    kv_set(&site, &format!("day:{}", ago(40)), &maximal_blob()).await;
    kv_set(&site, &format!("day:{}", ago(1)), &maximal_blob()).await;
    let host = host_over(Flaky::quota(&site, usage(&site).await));
    host.dispatch_action(PLUGIN, "request.served", payload(vec![view(ago(0), "/")]))
        .await
        .unwrap();
    assert_eq!(days(&site).await, [ago(1), ago(0)]);
    assert_eq!(totals(&site, ago(40)).await.unwrap()["views"], 1500);
    assert_eq!(day(&site, ago(0)).await.unwrap()["views"], 1);
    site
}

/// A compaction write refused for a reason other than the quota (a store error, which may
/// pass) keeps the full blob for a later batch rather than deleting it to make room.
async fn a_compaction_refused_for_another_reason_keeps_the_full_day(site: Site) -> Site {
    let site = site_with_analytics(site).await;
    kv_set(&site, &format!("day:{}", ago(40)), &maximal_blob()).await;
    let host = host_over(Flaky {
        inner: site.services.clone(),
        failing_get: Vec::new(),
        failing_set: vec!["totals:".to_owned()],
        quota: None,
    });
    host.dispatch_action(PLUGIN, "request.served", payload(vec![view(ago(0), "/")]))
        .await
        .unwrap();
    assert_eq!(days(&site).await, [ago(40), ago(0)]);
    assert!(totals(&site, ago(40)).await.is_none());

    // With the store working again, the next batch compacts it.
    feed(&site, vec![view(ago(0), "/")]).await;
    assert_eq!(days(&site).await, [ago(0)]);
    assert_eq!(totals(&site, ago(40)).await.unwrap()["views"], 1500);
    site
}

/// A-1: the page lists which days have a blob and reads only those; a listed day whose
/// read answers `None` failed to read, and the page says so instead of counting it as
/// empty. With every stored day unreadable it says the statistics could not be read.
async fn the_page_says_which_days_could_not_be_read(site: Site) -> Site {
    let site = viewer_site(site).await;
    feed(
        &site,
        vec![view(d1(), "/"), view(d2(), "/"), view(d2(), "/")],
    )
    .await;
    let host = host_over(Flaky::failing_get(
        site.services.clone(),
        &format!("day:{}", d1()),
    ));
    let page = |days: &'static str| {
        let host = &host;
        async move {
            let res = host
                .handle_route(PLUGIN, request(Some("v"), &[("days", days)]))
                .await
                .unwrap();
            (res.status, String::from_utf8(res.body).unwrap())
        }
    };
    let (status, html) = page("7").await;
    assert_eq!(status, 200);
    assert_eq!(stat(&html, "views"), 2);
    assert!(
        html.contains("1 day could not be read right now and is left out"),
        "{html}"
    );

    // Every stored day unreadable.
    let host = host_over(Flaky {
        inner: site.services.clone(),
        failing_get: vec![format!("day:{}", d1()), format!("day:{}", d2())],
        failing_set: Vec::new(),
        quota: None,
    });
    let res = host
        .handle_route(PLUGIN, request(Some("v"), &[("days", "7")]))
        .await
        .unwrap();
    let html = String::from_utf8(res.body).unwrap();
    assert!(html.contains("could not be read right now"), "{html}");
    assert!(!html.contains("No data yet"), "{html}");
    assert!(!html.contains("data-stat"), "{html}");

    // The plugin's own host, reading normally: no note.
    let (_, html) = stats(&site, &[("days", "7")]).await;
    assert_eq!(stat(&html, "views"), 3);
    assert!(!html.contains("data-note=\"unread\""), "{html}");
    site
}

/// A-6: core sends paths percent-encoded; the 200-byte cut never leaves half an escape at
/// the end of a key, and the page shows a path decoded when that is readable text (still
/// escaped), and as stored when it is not.
async fn encoded_paths_are_cut_whole_and_shown_decoded(site: Site) -> Site {
    let site = viewer_site(site).await;
    // 4 + 6 x 32 = 196 bytes, then `%D0%` up to byte 200: cut back to `%D0`.
    feed(&site, vec![long_view(d2(), 7)]).await;
    feed(
        &site,
        vec![
            core_event("/café/", 200),
            core_event("/a<b>/", 200),
            core_event("/x/%FF/", 200),
        ],
    )
    .await;
    let blob = day(&site, d2()).await.unwrap();
    let key = blob["paths"]
        .as_object()
        .unwrap()
        .keys()
        .next()
        .unwrap()
        .clone();
    assert_eq!(key.len(), 199, "{key}");
    assert!(key.ends_with("%D0"), "{key}");
    let today = day(&site, ago(0)).await.unwrap();
    let paths = today["paths"].as_object().unwrap();
    assert!(paths.contains_key("/caf%C3%A9/"), "{paths:?}");
    assert!(paths.contains_key("/a%3Cb%3E/"), "{paths:?}");

    let (_, html) = stats(&site, &[("days", "7")]).await;
    assert!(html.contains("<code>/café/</code>"), "{html}");
    assert!(html.contains("<code>/a&lt;b&gt;/</code>"), "{html}");
    assert!(!html.contains("<b>"), "{html}");
    // Not UTF-8 once decoded: shown as stored.
    assert!(html.contains("<code>/x/%FF/</code>"), "{html}");
    site
}

/// The dearest batch with the most maintenance: 30 expired full days to delete, the eight
/// compactions of maximal blobs a batch may do, 100 expired compacted days to list (and as
/// many deleted as operations allow), then 1000 long views over three days whose oldest
/// holds a maximal blob. At the default fuel (`ANALYTICS_FUEL` brackets it).
async fn the_worst_batch_with_maintenance_fits_the_default_fuel(site: Site) -> Site {
    let site = site_with_analytics(site).await;
    for n in 0..30 {
        kv_set(&site, &format!("day:{}", ago(100 + n)), STUB).await;
    }
    for n in 31..=38 {
        kv_set(&site, &format!("day:{}", ago(n)), &maximal_blob()).await;
    }
    for n in 0..100 {
        kv_set(&site, &format!("totals:{}", ago(200 + n)), STUB).await;
    }
    kv_set(&site, &format!("day:{}", ago(3)), &maximal_blob()).await;
    let events = (0..1000)
        .map(|i| long_view(ago([1, 2, 3, 4][i % 4]), i))
        .collect();
    feed(&site, events).await;
    assert_eq!(day(&site, ago(3)).await.unwrap()["views"], 1500 + 250);
    assert_eq!(day(&site, ago(1)).await.unwrap()["views"], 250);
    let compacted = totals_days(&site).await;
    for n in 31..=38 {
        assert!(compacted.contains(&ago(n).to_owned()), "{}", ago(n));
    }
    assert!(days(&site).await.iter().all(|d| d.as_str() > ago(39)));
    site
}
