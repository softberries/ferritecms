//! `GET /api/v1/themes` reports a node's startup fallback (M2a-2 Task 5,
//! `theme_list_shows_node_state`'s real-binary half; P12, pre-flight F12): a theme
//! uploaded and activated over the API serves after a restart with `fallback: false`;
//! after its stored bundle is tampered with, the next start falls back to the embedded
//! `blog`, keeps serving, and the active entry's node state says `fallback: true`.

use std::time::{Duration, Instant};

use ferrite_theme::{ThemeSource, pack};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::harness::{Profile, Resp, SITE_URL, Server, request, request_bytes};

const PASSWORD: &str = "correct horse battery staple";
const THEMES: &str = "/api/v1/themes";
const MARK: &str = "MARK-THEME-API";

struct Session {
    cookie: String,
    csrf: String,
}

fn login(server: &Server) -> Session {
    let r = request(
        server.addr,
        "POST",
        "/api/v1/auth/login",
        &[("Origin", SITE_URL.to_owned())],
        Some(&json!({"login": "admin", "password": PASSWORD})),
    );
    assert_eq!(r.status, 200, "{r:?}");
    let cookie = r
        .header("set-cookie")
        .and_then(|c| c.split(';').next())
        .and_then(|kv| kv.strip_prefix("ferrite_session="))
        .expect("session cookie")
        .to_owned();
    let csrf = r.json()["csrf_token"].as_str().expect("csrf").to_owned();
    Session { cookie, csrf }
}

fn headers(s: &Session) -> Vec<(&'static str, String)> {
    vec![
        ("Origin", SITE_URL.to_owned()),
        ("Cookie", format!("ferrite_session={}", s.cookie)),
        ("X-CSRF-Token", s.csrf.clone()),
    ]
}

/// A packed theme `marked` whose pages contain [`MARK`].
fn bundle() -> Vec<u8> {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("templates")).unwrap();
    std::fs::write(
        dir.path().join("theme.toml"),
        "[theme]\nname = \"marked\"\nversion = \"0.3.0\"\nlicense = \"MIT\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("templates/index.html"),
        format!("<!doctype html><html><head>{{{{ head() }}}}</head><body>{MARK}</body></html>"),
    )
    .unwrap();
    pack(&ThemeSource::Dir(dir.path().to_owned())).unwrap()
}

/// The `marked` entry of `GET /themes` once at least one live node reports a theme.
fn marked_with_nodes(server: &Server, s: &Session) -> Value {
    let start = Instant::now();
    loop {
        let r = request(server.addr, "GET", THEMES, &headers(s), None);
        assert_eq!(r.status, 200, "{r:?}");
        let list = r.json();
        let entry = list["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "marked")
            .cloned()
            .unwrap_or_else(|| panic!("no marked entry: {list}"));
        if entry["nodes"].as_array().is_some_and(|n| !n.is_empty()) {
            return entry;
        }
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "no node state: {list}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn home(server: &Server) -> Resp {
    let r = server.get("/");
    assert_eq!(r.status, 200, "{r:?}");
    r
}

fn theme_fallback_is_listed(p: Profile) {
    p.init();
    p.create_admin("admin", PASSWORD);
    let bytes = bundle();
    let hash: String = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();

    // Upload and activate over the API.
    let server = Server::start(&p);
    let s = login(&server);
    let mut h = headers(&s);
    h.push(("Content-Type", "application/x-tar".to_owned()));
    let r = request_bytes(
        server.addr,
        "POST",
        &format!("{THEMES}?activate=true"),
        &h,
        &bytes,
    );
    assert_eq!(r.status, 201, "{r:?}");
    assert_eq!(r.json()["active"], true, "{r:?}");
    assert!(server.stop_with("TERM").success());

    // A restart serves the artifact: not a fallback.
    let server = Server::start(&p);
    let s = login(&server);
    let entry = marked_with_nodes(&server, &s);
    assert_eq!(entry["active"], true, "{entry}");
    for node in entry["nodes"].as_array().unwrap() {
        assert_eq!(node["fallback"], false, "{entry}");
        assert_eq!(node["origin"], "artifact", "{entry}");
        assert_eq!(node["name"], "marked", "{entry}");
    }
    assert!(home(&server).text().contains(MARK));
    assert!(server.stop_with("TERM").success());

    // The stored bundle is tampered with: the next start falls back, keeps serving, and
    // says so.
    let blob = p.artifact_blob(&hash);
    assert!(blob.is_file(), "no blob at {}", blob.display());
    std::fs::write(&blob, b"tampered").unwrap();
    let server = Server::start(&p);
    let s = login(&server);
    let entry = marked_with_nodes(&server, &s);
    assert_eq!(
        entry["active"], true,
        "the row is still the active one: {entry}"
    );
    let nodes = entry["nodes"].as_array().unwrap();
    let fallen = nodes
        .iter()
        .find(|n| n["fallback"] == true)
        .unwrap_or_else(|| panic!("no fallback node: {entry}"));
    assert_eq!(fallen["name"], "blog", "{entry}");
    assert_eq!(fallen["origin"], "embedded", "{entry}");
    assert!(
        fallen["error"]
            .as_str()
            .is_some_and(|e| e.contains("hash mismatch")),
        "{entry}"
    );
    let page = home(&server).text();
    assert!(
        page.contains("class=\"skip-link\"") && !page.contains(MARK),
        "{page}"
    );

    // Activating it again over the API is refused; the fallback keeps serving.
    let r = request(
        server.addr,
        "POST",
        &format!("{THEMES}/marked/activate"),
        &headers(&s),
        None,
    );
    r.problem(422, "validation");
    assert!(home(&server).text().contains("class=\"skip-link\""));
    assert!(server.stop_with("TERM").success());
}

#[test]
fn theme_fallback_is_listed_solo() {
    theme_fallback_is_listed(Profile::solo());
}

#[test]
fn theme_fallback_is_listed_cluster() {
    if let Some(p) = Profile::cluster() {
        theme_fallback_is_listed(p);
    }
}
