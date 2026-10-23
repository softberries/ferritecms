//! The `/api/v1` API end to end against the real `ferrite` binary (M2a-1 task 13), on
//! both profiles. `init` → `user create --password-stdin` → `serve` on a free port, then
//! a browser-like session (cookie + CSRF, `Origin` = `site.url`, F29) and bearer tokens
//! over plain HTTP/1.1.

use std::net::SocketAddr;
use std::sync::Mutex;

use serde_json::{Value, json};

use crate::harness::{Profile, Resp, SITE_URL, Server, request};

const ADMIN_PASSWORD: &str = "correct horse battery staple";
const EDITOR_PASSWORD: &str = "editor passphrase 2026";
const ME: &str = "/api/v1/auth/me";
/// Tokens a user can hold (constraints: at most 50).
const MAX_TOKENS: usize = 50;

/// Every response of the run, for the secret checks at the end, with the secrets that
/// response legitimately shows: a token in its creation response, a session's CSRF
/// token in its login and `me` responses.
#[derive(Default)]
struct Seen {
    responses: Vec<(Resp, Vec<String>)>,
    tokens: Vec<String>,
    csrf: Vec<String>,
    cookies: Vec<String>,
}

/// How a request authenticates.
#[derive(Clone, Copy)]
enum Auth<'a> {
    None,
    /// A browser: `Origin` = `site.url`, the cookie and (unsafe methods) the CSRF token.
    Browser(&'a Browser),
    Bearer(&'a str),
}

#[derive(Default, Clone)]
struct Browser {
    cookie: Option<String>,
    csrf: Option<String>,
}

/// The site under test plus the response log.
struct Site {
    addr: SocketAddr,
    seen: Mutex<Seen>,
}

impl Site {
    fn send(&self, auth: Auth<'_>, method: &str, path: &str, body: Option<&Value>) -> Resp {
        self.send_with(auth, method, path, body, &[])
    }

    fn send_with(
        &self,
        auth: Auth<'_>,
        method: &str,
        path: &str,
        body: Option<&Value>,
        extra: &[(&str, String)],
    ) -> Resp {
        let mut headers: Vec<(&str, String)> = extra.to_vec();
        match auth {
            Auth::None => {}
            Auth::Browser(b) => {
                headers.push(("Origin", SITE_URL.to_owned()));
                if let Some(c) = &b.cookie {
                    headers.push(("Cookie", format!("ferrite_session={c}")));
                }
                if let (false, Some(t)) = (matches!(method, "GET" | "HEAD"), &b.csrf) {
                    headers.push(("X-CSRF-Token", t.clone()));
                }
            }
            Auth::Bearer(t) => headers.push(("Authorization", format!("Bearer {t}"))),
        }
        let r = request(self.addr, method, path, &headers, body);
        let copy = r.clone();
        let shown = match auth {
            Auth::Browser(b) if path == ME => b.csrf.iter().cloned().collect(),
            _ => Vec::new(),
        };
        self.seen.lock().unwrap().responses.push((copy, shown));
        r
    }

    /// `POST /auth/login` as a browser; the cookie and CSRF token on success.
    fn login(&self, login: &str, password: &str) -> (Resp, Browser) {
        let r = self.send(
            Auth::Browser(&Browser::default()),
            "POST",
            "/api/v1/auth/login",
            Some(&json!({"login": login, "password": password})),
        );
        let mut b = Browser::default();
        if r.status == 200 {
            let set = r.header("set-cookie").expect("session cookie");
            assert!(set.contains("HttpOnly"), "{set}");
            assert!(set.contains("SameSite=Lax"), "{set}");
            assert!(!set.contains("Secure"), "http site.url: no Secure ({set})");
            let value = set
                .split(';')
                .next()
                .and_then(|kv| kv.strip_prefix("ferrite_session="))
                .expect("ferrite_session")
                .to_owned();
            let csrf = r.json()["csrf_token"]
                .as_str()
                .expect("csrf_token")
                .to_owned();
            let mut seen = self.seen.lock().unwrap();
            seen.cookies.push(value.clone());
            seen.csrf.push(csrf.clone());
            seen.responses.last_mut().unwrap().1.push(csrf.clone());
            b.cookie = Some(value);
            b.csrf = Some(csrf);
        }
        (r, b)
    }

    /// Creates a token for `user` as `who`; returns its id and secret.
    fn create_token(&self, who: &Browser, user: &str, name: &str) -> Resp {
        let r = self.send(
            Auth::Browser(who),
            "POST",
            &format!("/api/v1/users/{user}/tokens"),
            Some(&json!({"name": name, "scopes": ["content:read"]})),
        );
        if r.status == 201 {
            let secret = r.json()["token"].as_str().unwrap().to_owned();
            let mut seen = self.seen.lock().unwrap();
            seen.tokens.push(secret.clone());
            seen.responses.last_mut().unwrap().1.push(secret);
        }
        r
    }

    /// No response — refusals included — carries a password, hash, TOTP or CSRF secret,
    /// a session cookie value, a token secret other than in its creation response, or a
    /// CSRF token other than in its own session's login and `me` responses.
    fn assert_no_secrets(&self) {
        let seen = self.seen.lock().unwrap();
        assert!(
            seen.responses.len() > 80,
            "{} responses",
            seen.responses.len()
        );
        for (r, shown) in &seen.responses {
            let body = r.text();
            for needle in [
                "password_hash",
                "totp_secret",
                "csrf_secret",
                "token_hash",
                "$argon2",
                "\"password\"",
                ADMIN_PASSWORD,
                EDITOR_PASSWORD,
            ] {
                assert!(!body.contains(needle), "{needle} in {r:?}");
            }
            for cookie in &seen.cookies {
                assert!(!body.contains(cookie.as_str()), "cookie in {r:?}");
            }
            for token in seen.tokens.iter().filter(|t| !shown.contains(t)) {
                assert!(!body.contains(token.as_str()), "token in {r:?}");
                assert!(!body.contains(&token[4..]), "token in {r:?}");
            }
            for csrf in seen.csrf.iter().filter(|t| !shown.contains(t)) {
                assert!(!body.contains(csrf.as_str()), "CSRF token in {r:?}");
            }
        }
    }
}

/// The `unauthenticated` problem with its `WWW-Authenticate: Bearer` challenge (R14).
fn unauthenticated(r: Resp) {
    r.problem(401, "unauthenticated");
    assert!(
        r.header("www-authenticate")
            .is_some_and(|v| v.starts_with("Bearer")),
        "{r:?}"
    );
}

fn api_end_to_end_on_real_binary(p: Profile) {
    p.init();
    p.create_admin("admin", ADMIN_PASSWORD);
    let server = Server::start(&p);
    let site = Site {
        addr: server.addr,
        seen: Mutex::default(),
    };
    let me = ME;

    // Anonymous: 401; a login from a foreign origin is refused before the password.
    unauthenticated(site.send(Auth::None, "GET", me, None));
    let r = site.send_with(
        Auth::None,
        "POST",
        "/api/v1/auth/login",
        Some(&json!({"login": "admin", "password": ADMIN_PASSWORD})),
        &[("Origin", "http://evil.example".to_owned())],
    );
    assert_eq!(r.status, 403, "{r:?}");
    assert!(r.header("set-cookie").is_none(), "{r:?}");

    // A wrong password and an unknown login get the same uniform 401, and no cookie.
    let (wrong, _) = site.login("admin", "not the admin password");
    let (unknown, _) = site.login("nobody", "not the admin password");
    for r in [&wrong, &unknown] {
        r.problem(401, "invalid-credentials");
        assert!(r.header("set-cookie").is_none(), "{r:?}");
        assert!(r.header("www-authenticate").is_some(), "{r:?}");
    }
    assert_eq!(wrong.body, unknown.body);

    // Login: the cookie and the CSRF token.
    let (r, admin) = site.login("admin", ADMIN_PASSWORD);
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(r.header("cache-control"), Some("no-store"));
    let login = r.json();
    assert_eq!(login["status"], "session");
    assert_eq!(login["user"]["login"], "admin");
    assert_eq!(login["user"]["role"], "admin");
    let r = site.send(Auth::Browser(&admin), "GET", me, None);
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(r.json()["login"], "admin");

    // A cookie write without the CSRF token is refused and writes nothing.
    let no_csrf = Browser {
        csrf: None,
        ..admin.clone()
    };
    let post = json!({"title": "End To End Post"});
    let r = site.send(
        Auth::Browser(&no_csrf),
        "POST",
        "/api/v1/content/post",
        Some(&post),
    );
    assert_eq!(r.status, 403, "{r:?}");

    // Create and publish (If-Match) a post; the public permalink shows it.
    let r = site.send(
        Auth::Browser(&admin),
        "POST",
        "/api/v1/content/post",
        Some(&post),
    );
    assert_eq!(r.status, 201, "{r:?}");
    let created = r.json();
    assert_eq!(created["status"], "draft");
    let id = created["id"].as_str().unwrap().to_owned();
    let version = created["version"].as_i64().unwrap();
    let publish = format!("/api/v1/content/post/{id}/publish");
    site.send(Auth::Browser(&admin), "POST", &publish, None)
        .problem(428, "precondition-required");
    let r = site.send_with(
        Auth::Browser(&admin),
        "POST",
        &publish,
        None,
        &[("If-Match", format!("\"{version}\""))],
    );
    assert_eq!(r.status, 200, "{r:?}");
    let published = r.json();
    assert_eq!(published["status"], "published");
    let permalink = published["permalink"].as_str().unwrap().to_owned();
    assert!(permalink.starts_with('/'), "{permalink}");
    let r = site.send(Auth::None, "GET", &permalink, None);
    assert_eq!(r.status, 200, "{r:?}");
    assert!(r.text().contains("End To End Post"), "{}", r.text());
    assert!(
        r.header("cache-control") != Some("private, no-store"),
        "{r:?}"
    );

    // The same page logged in: rendered for the user, never cached.
    let r = site.send(Auth::Browser(&admin), "GET", &permalink, None);
    assert_eq!(r.status, 200, "{r:?}");
    assert!(r.text().contains("End To End Post"), "{}", r.text());
    assert_eq!(r.header("cache-control"), Some("private, no-store"));

    // A second post stays a draft: the preview needs the cookie.
    let r = site.send(
        Auth::Browser(&admin),
        "POST",
        "/api/v1/content/post",
        Some(&json!({"title": "Unpublished Preview Draft"})),
    );
    assert_eq!(r.status, 201, "{r:?}");
    let draft = r.json()["id"].as_str().unwrap().to_owned();
    let preview = format!("/_preview/{draft}");
    let r = site.send(Auth::Browser(&admin), "GET", &preview, None);
    assert_eq!(r.status, 200, "{r:?}");
    assert!(
        r.text().contains("Unpublished Preview Draft"),
        "{}",
        r.text()
    );
    assert_eq!(r.header("cache-control"), Some("private, no-store"));
    assert_eq!(r.header("x-robots-tag"), Some("noindex"));
    let r = site.send(Auth::None, "GET", &preview, None);
    assert_eq!(r.status, 404, "{r:?}");
    assert!(!r.text().contains("Unpublished Preview Draft"));

    // An editor account, logged in.
    let r = site.send(
        Auth::Browser(&admin),
        "POST",
        "/api/v1/users",
        Some(&json!({
            "login": "eddie", "email": "eddie@example.com", "display_name": "Eddie",
            "role": "editor", "password": EDITOR_PASSWORD
        })),
    );
    assert_eq!(r.status, 201, "{r:?}");
    let eddie = r.json()["id"].as_str().unwrap().to_owned();
    let (r, editor) = site.login("eddie", EDITOR_PASSWORD);
    assert_eq!(r.status, 200, "{r:?}");
    let tokens = format!("/api/v1/users/{eddie}/tokens");

    // The editor's token works; the admin lists and revokes it, and it stops at once.
    let r = site.create_token(&editor, &eddie, "laptop");
    assert_eq!(r.status, 201, "{r:?}");
    let first = r.json();
    let first_secret = first["token"].as_str().unwrap().to_owned();
    let first_id = first["id"].as_str().unwrap().to_owned();
    let r = site.send(Auth::Bearer(&first_secret), "GET", me, None);
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(r.json()["scopes"], json!(["content:read"]));
    let r = site.send(Auth::Browser(&admin), "GET", &tokens, None);
    assert_eq!(r.status, 200, "{r:?}");
    let listed = r.json();
    assert_eq!(listed["items"].as_array().unwrap().len(), 1, "{listed}");
    assert_eq!(listed["items"][0]["id"], first_id.as_str());
    let r = site.send(
        Auth::Browser(&admin),
        "DELETE",
        &format!("{tokens}/{first_id}"),
        None,
    );
    assert_eq!(r.status, 204, "{r:?}");
    unauthenticated(site.send(Auth::Bearer(&first_secret), "GET", me, None));
    let r = site.send(Auth::Browser(&admin), "GET", &tokens, None);
    assert_eq!(r.json()["items"], json!([]));

    // At most 50 tokens: the 51st is a validation problem.
    for i in 0..MAX_TOKENS {
        let r = site.create_token(&editor, &eddie, &format!("t{i}"));
        assert_eq!(r.status, 201, "token {i}: {r:?}");
    }
    let last_secret = site.seen.lock().unwrap().tokens.last().unwrap().clone();
    site.create_token(&editor, &eddie, "one too many")
        .problem(422, "validation");
    let r = site.send(Auth::Browser(&admin), "GET", &tokens, None);
    assert_eq!(r.json()["items"].as_array().unwrap().len(), MAX_TOKENS);

    // Disabling the editor rejects their cookie and their tokens on the next request.
    let r = site.send(Auth::Bearer(&last_secret), "GET", me, None);
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(
        site.send(Auth::Browser(&editor), "GET", me, None).status,
        200
    );
    let r = site.send(
        Auth::Browser(&admin),
        "PATCH",
        &format!("/api/v1/users/{eddie}"),
        Some(&json!({"status": "disabled"})),
    );
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(r.json()["status"], "disabled");
    unauthenticated(site.send(Auth::Browser(&editor), "GET", me, None));
    unauthenticated(site.send(Auth::Bearer(&last_secret), "GET", me, None));
    let (r, _) = site.login("eddie", EDITOR_PASSWORD);
    assert_ne!(r.status, 200, "a disabled account cannot log in: {r:?}");
    assert!(r.header("set-cookie").is_none(), "{r:?}");

    // Logout ends the session: the old cookie is 401.
    let r = site.send(Auth::Browser(&admin), "POST", "/api/v1/auth/logout", None);
    assert_eq!(r.status, 204, "{r:?}");
    let cleared = r.header("set-cookie").expect("cookie cleared");
    assert!(cleared.contains("Max-Age=0"), "{cleared}");
    unauthenticated(site.send(Auth::Browser(&admin), "GET", me, None));

    site.assert_no_secrets();
    let status = server.stop_with("TERM");
    assert!(status.success(), "serve exit: {status:?}");
}

#[test]
fn api_end_to_end_on_real_binary_solo() {
    api_end_to_end_on_real_binary(Profile::solo());
}

#[test]
fn api_end_to_end_on_real_binary_cluster() {
    if let Some(p) = Profile::cluster() {
        api_end_to_end_on_real_binary(p);
    }
}
