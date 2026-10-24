//! Batched `request.served` (M2a-2 Task 12, P21, F36): page responses, cache hits
//! included, become one `plugin.action` job per subscriber per flush; the payload holds
//! no IP, cookie, query or full user agent; assets, health checks, previews and plugin
//! routes are not recorded; the buffer is bounded and never blocks a request.

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    body::Body,
    http::{Method, Request, header},
};
use ferrite_core::jobs::PLUGIN_ACTION_KIND;
use ferrite_domain::{Job, NodeId};
use ferrite_http::{
    CacheOutcome, ClientIp, SERVED_BUFFER_CAP, SERVED_FLUSH_AT, ServedBuffer, ServedEvent,
};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::app_tests;
use crate::harness::App;

const UA: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:131.0) Gecko/20100101 Firefox/131.0";
const IP: &str = "198.51.100.23";
const COOKIE: &str = "ferrite_session=not-a-real-session; theme=dark";

/// GET `path` as a browser would send it: user agent, referrer, cookie, client address.
fn browser(path: &str) -> Request<Body> {
    let mut req = Request::builder()
        .method(Method::GET)
        .uri(path)
        .header(header::HOST, "example.com")
        .header(header::USER_AGENT, UA)
        .header(
            header::REFERER,
            "https://Search.Example.ORG/results?q=private",
        )
        .header(header::COOKIE, COOKIE)
        .header("x-forwarded-for", IP)
        .body(Body::empty())
        .expect("request");
    req.extensions_mut()
        .insert(ClientIp(IP.parse::<IpAddr>().expect("ip")));
    req
}

/// Installs and loads the `counter` example (subscribes to `request.served`).
async fn with_counter(app: &App) {
    app.site.install("counter").await;
    let loaded = app.site.load().await;
    assert!(app.site.host.is_enabled("counter"), "{loaded:?}");
}

/// Every queued `plugin.action` job (claimed, so each test calls this once per batch).
async fn action_jobs(app: &App) -> Vec<Job> {
    app.site
        .store
        .read()
        .jobs()
        .claim(
            NodeId::new(),
            &[PLUGIN_ACTION_KIND.to_owned()],
            100,
            Duration::from_secs(60),
        )
        .await
        .expect("claim")
}

fn events(job: &Job) -> &Vec<Value> {
    job.payload["payload"]["events"]
        .as_array()
        .expect("events array")
}

/// Generous bound for the flusher to react (a slow CI runner).
const FLUSH_WAIT: Duration = Duration::from_secs(30);

fn event(path: &str) -> ServedEvent {
    ServedEvent::new(path, &Default::default(), 200, CacheOutcome::Miss)
}

app_tests!(
    served_events_batched_into_one_job,
    excluded_requests_are_not_recorded,
    logged_in_bypass_is_recorded_as_bypass,
    flush_at_1000_events,
    no_subscriber_no_job,
    buffer_drops_beyond_cap,
);

async fn served_events_batched_into_one_job(app: App) -> App {
    let (app, served) = app.with_served().await;
    with_counter(&app).await;
    app.post("Hello", "hello-world", &[]).await;

    // Miss, hit, miss, hit, and a 404 (never cached: a render each time).
    for path in [
        "/",
        "/",
        "/hello-world/?utm_source=secret",
        "/hello-world/",
        "/nope/",
    ] {
        app.send(browser(path)).await;
    }
    assert_eq!(served.len(), 5);
    assert_eq!(served.flush().await.expect("flush"), 1);
    assert!(served.is_empty());

    let jobs = action_jobs(&app).await;
    assert_eq!(jobs.len(), 1, "{jobs:?}");
    let job = &jobs[0];
    assert_eq!(job.priority, -1);
    assert_eq!(job.payload["plugin"], "counter");
    assert_eq!(job.payload["hook"], "request.served");
    assert_eq!(job.payload["subject"], Value::Null);
    let evs = events(job);
    let paths: Vec<&str> = evs.iter().map(|e| e["path"].as_str().unwrap()).collect();
    assert_eq!(
        paths,
        ["/", "/", "/hello-world/", "/hello-world/", "/nope/"]
    );
    let cache: Vec<&str> = evs.iter().map(|e| e["cache"].as_str().unwrap()).collect();
    assert_eq!(cache, ["miss", "hit", "miss", "hit", "miss"]);
    let status: Vec<u64> = evs.iter().map(|e| e["status"].as_u64().unwrap()).collect();
    assert_eq!(status, [200, 200, 200, 200, 404]);
    for e in evs {
        assert_eq!(e["ua_class"], "browser");
        assert_eq!(e["referrer_host"], "search.example.org");
        assert!(e["ts"].as_str().is_some_and(|t| t.ends_with('Z')), "{e}");
        let keys: Vec<&String> = e.as_object().unwrap().keys().collect();
        assert_eq!(
            keys.len(),
            6,
            "only path, status, ts, referrer_host, ua_class, cache: {e}"
        );
    }
    // Nothing identifying anywhere in the job.
    let text = job.payload.to_string();
    for secret in [
        IP,
        "198.51.100",
        "not-a-real-session",
        "theme=dark",
        "Firefox",
        "Gecko",
        "utm_source",
        "private",
    ] {
        assert!(!text.contains(secret), "{secret:?} in {text}");
    }
    app
}

/// Health checks, assets, previews, plugin routes and non-GET requests are not page
/// responses; redirects are (bypass).
async fn excluded_requests_are_not_recorded(app: App) -> App {
    let (app, served) = app.with_served().await;
    with_counter(&app).await;
    let hash = "ab".repeat(32);
    for path in [
        "/healthz",
        "/readyz",
        &format!("/_media/{hash}/x.png"),
        &format!("/_theme/{hash}/css/site.css"),
        "/_preview/00000000-0000-0000-0000-000000000000",
        // Preview variants the router answers with a 404 page: still not recorded.
        "/_preview/00000000-0000-0000-0000-000000000000/",
        "/_preview/00000000-0000-0000-0000-000000000000/x",
        "/_Preview/00000000-0000-0000-0000-000000000000",
        "/_PREVIEW/",
        "/%5Fpreview/00000000-0000-0000-0000-000000000000",
        "/_%70review/00000000-0000-0000-0000-000000000000",
        "/_p/counter/anything",
    ] {
        app.send(browser(path)).await;
    }
    let mut post = browser("/");
    *post.method_mut() = Method::POST;
    app.send(post).await;
    assert_eq!(served.len(), 0, "nothing recorded");

    // The canonical redirect is a page response answered before the cache.
    let r = app.send(browser("/hello-world")).await;
    assert_eq!(r.status.as_u16(), 301, "{r:?}");
    assert_eq!(served.len(), 1);
    served.flush().await.expect("flush");
    let jobs = action_jobs(&app).await;
    let evs = events(&jobs[0]);
    assert_eq!(evs[0]["status"], 301);
    assert_eq!(evs[0]["cache"], "bypass");
    app
}

async fn logged_in_bypass_is_recorded_as_bypass(app: App) -> App {
    let (app, served) = app.with_served().await;
    with_counter(&app).await;
    let cookie = app.session(&app.ada_user).await;
    let mut req = browser("/");
    req.headers_mut()
        .insert(header::COOKIE, cookie.parse().expect("cookie"));
    app.send(req).await;
    served.flush().await.expect("flush");
    let jobs = action_jobs(&app).await;
    let evs = events(&jobs[0]);
    assert_eq!(evs.len(), 1);
    assert_eq!(evs[0]["cache"], "bypass");
    // No user identity in the event.
    let text = jobs[0].payload.to_string();
    assert!(!text.contains(&app.ada_user.id.to_string()), "{text}");
    assert!(!text.contains("ada"), "{text}");
    app
}

/// The flusher flushes at 1 000 buffered events without waiting for its interval, and
/// once more when stopped.
async fn flush_at_1000_events(app: App) -> App {
    with_counter(&app).await;
    let served = Arc::new(ServedBuffer::new(
        app.site.store.clone(),
        app.site.host.clone(),
    ));
    let stop = CancellationToken::new();
    let flusher = served
        .clone()
        .spawn_flusher(Duration::from_secs(3600), stop.clone());
    for i in 0..SERVED_FLUSH_AT - 1 {
        served.record(event(&format!("/p/{i}")));
    }
    // Below 1 000 only the (1 h) tick could flush; the notify fires at exactly 1 000.
    assert!(action_jobs(&app).await.is_empty(), "no flush below 1 000");
    assert_eq!(served.len(), SERVED_FLUSH_AT - 1);
    served.record(event("/last"));
    // `flush` takes the buffer, then commits: wait for the job itself (claimed once).
    let deadline = tokio::time::Instant::now() + FLUSH_WAIT;
    let jobs = loop {
        let jobs = action_jobs(&app).await;
        if !jobs.is_empty() {
            break jobs;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no job after the 1 000th event within {FLUSH_WAIT:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert!(served.is_empty());
    assert_eq!(jobs.len(), 1, "{jobs:?}");
    assert_eq!(events(&jobs[0]).len(), SERVED_FLUSH_AT);

    // The final flush on stop.
    for path in ["/a", "/b", "/c"] {
        served.record(event(path));
    }
    stop.cancel();
    flusher.await.expect("flusher");
    let jobs = action_jobs(&app).await;
    assert_eq!(jobs.len(), 1, "{jobs:?}");
    assert_eq!(events(&jobs[0]).len(), 3);
    app
}

async fn no_subscriber_no_job(app: App) -> App {
    let (app, served) = app.with_served().await;
    // A loaded plugin that does not subscribe.
    app.site.install("hello").await;
    app.site.load().await;
    assert!(app.site.host.is_enabled("hello"));
    app.send(browser("/")).await;
    assert_eq!(served.len(), 1);
    assert_eq!(served.flush().await.expect("flush"), 0);
    assert!(served.is_empty(), "discarded without subscribers");
    assert!(action_jobs(&app).await.is_empty());

    // A subscriber but no events.
    with_counter(&app).await;
    assert_eq!(served.flush().await.expect("flush"), 0);
    assert!(action_jobs(&app).await.is_empty());
    app
}

async fn buffer_drops_beyond_cap(app: App) -> App {
    with_counter(&app).await;
    let served = ServedBuffer::new(app.site.store.clone(), app.site.host.clone());
    for i in 0..SERVED_BUFFER_CAP + 5 {
        served.record(event(&format!("/p/{i}")));
    }
    assert_eq!(served.len(), SERVED_BUFFER_CAP);
    assert_eq!(served.dropped(), 5);
    // At most 1 000 events per job.
    assert_eq!(served.flush().await.expect("flush"), 10);
    let jobs = action_jobs(&app).await;
    assert_eq!(jobs.len(), 10);
    assert!(jobs.iter().all(|j| events(j).len() == SERVED_FLUSH_AT));
    // Recording continues after a flush.
    served.record(event("/again"));
    assert_eq!(served.len(), 1);
    assert_eq!(served.dropped(), 5);
    app
}
