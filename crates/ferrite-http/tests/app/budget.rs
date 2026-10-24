//! Plugin call limits on real pages (M2a-2 Task 13, P22): the per-render budget and the
//! call cap.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use ferrite_cache::CacheKey;
use ferrite_http::{HttpConfig, app::SHORT_TTL, build_app};
use ferrite_plugin_host::{HostConfig, PluginHost};

use crate::app_tests;
use crate::common::{SITE_URL, SlowPosts};
use crate::harness::{App, SITE_TITLE, fixture, load_theme};

/// This app with `host` as its plugin host and `config`.
pub async fn with_host(mut app: App, host: Arc<PluginHost>, config: HttpConfig) -> App {
    app.site.host = host;
    app.state = App::state_with(&app.site, load_theme(fixture("app-theme")), true, config).await;
    app.router = build_app(app.state.clone());
    app
}

/// P22: a page whose filters are slow stops calling plugins once the render budget
/// (300 ms here) is spent and still answers, cached as volatile.
async fn render_budget_bounds_a_page(app: App) -> App {
    let slow = Arc::new(SlowPosts::new(
        app.site.services.clone(),
        Duration::from_secs(5),
    ));
    let host = slow.host(HostConfig::default(), &["slow-a", "slow-b", "slow-c"]);
    let mut config = HttpConfig::new(SITE_URL, SITE_TITLE, "en");
    config.render_budget = Duration::from_millis(300);
    let app = with_host(app, host, config).await;
    app.post("Slow", "slow", &[]).await;
    let start = Instant::now();
    let r = app.get("/slow/").await;
    let took = start.elapsed();
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert!(r.body.contains("Slow"), "{}", r.body);
    assert!(took < Duration::from_millis(600), "took {took:?}");
    assert_eq!(slow.calls(), 1, "slow-b and slow-c were skipped");
    let cached = app
        .state
        .cache
        .get(&CacheKey::anon("example.com", "/slow/", ""))
        .await
        .expect("cached");
    assert_eq!(cached.ttl, Some(SHORT_TTL), "volatile");
    app
}

app_tests!(render_budget_bounds_a_page);
