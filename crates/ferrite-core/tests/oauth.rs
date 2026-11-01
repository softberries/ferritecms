//! The OAuth 2.1 authorization server (design 12 §3.2) on both backends: clients from
//! metadata documents and dynamic registration, authorization requests, consent, codes,
//! grants with refresh-token rotation, revocation and grant-bound access tokens.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use ferrite_core::{
    Actor, CoreError, NewAccount, Scopes, UserService,
    auth::{AuthService, SiteKey, Via},
    oauth::{
        AuthorizeError, AuthorizeParams, ClientMetadataFetcher, ClientRegistration, FetchError,
        OAuthError, OAuthService, TokenRequest, TokenResponse,
    },
};
use ferrite_domain::{AuditQuery, ClientKind, OAuthClient, OAuthClientId, Role, Timestamp, User};
use ferrite_store_tests::{TestDb, backend_tests};
use serde_json::json;
use url::Url;

const KEY: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
const SITE: &str = "https://site.example";
/// RFC 7636 Appendix B.
const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
const CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
const REDIRECT: &str = "http://127.0.0.1:33418/callback";
const DOC_URL: &str = "https://app.example/oauth/client.json";
const HOUR: Duration = Duration::from_secs(3600);

/// A test clock in microseconds.
#[derive(Clone)]
struct Clock(Arc<Mutex<i64>>);

impl Clock {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(Timestamp::now().as_micros())))
    }

    fn now(&self) -> Timestamp {
        Timestamp::from_micros(*self.0.lock().unwrap()).unwrap()
    }

    fn advance(&self, d: Duration) {
        *self.0.lock().unwrap() += i64::try_from(d.as_micros()).unwrap();
    }
}

/// Answers from a table; an unknown URL fails. Counts calls.
#[derive(Default)]
struct FakeFetcher {
    docs: Mutex<HashMap<String, Result<Vec<u8>, ()>>>,
    calls: AtomicU32,
}

impl FakeFetcher {
    fn serve(&self, url: &str, doc: serde_json::Value) {
        self.docs
            .lock()
            .unwrap()
            .insert(url.to_owned(), Ok(serde_json::to_vec(&doc).unwrap()));
    }

    fn fail(&self, url: &str) {
        self.docs.lock().unwrap().insert(url.to_owned(), Err(()));
    }

    fn calls(&self) -> u32 {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl ClientMetadataFetcher for FakeFetcher {
    async fn fetch(&self, url: &Url) -> Result<Vec<u8>, FetchError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match self.docs.lock().unwrap().get(url.as_str()) {
            Some(Ok(bytes)) => Ok(bytes.clone()),
            _ => Err(FetchError::Failed("connection refused".into())),
        }
    }
}

struct Env {
    oauth: OAuthService,
    auth: Arc<AuthService>,
    fetcher: Arc<FakeFetcher>,
    clock: Clock,
}

fn env(db: &TestDb) -> Env {
    let clock = Clock::new();
    let c = clock.clone();
    let auth = Arc::new(
        AuthService::new(db.shared_store(), SiteKey::from_hex(KEY).unwrap())
            .with_clock(Arc::new(move || c.now())),
    );
    let fetcher = Arc::new(FakeFetcher::default());
    let oauth = OAuthService::new(
        db.shared_store(),
        auth.clone(),
        fetcher.clone(),
        Url::parse(&format!("{SITE}/")).unwrap(),
    );
    Env {
        oauth,
        auth,
        fetcher,
        clock,
    }
}

fn ip(last: u8) -> Option<IpAddr> {
    Some(IpAddr::V4(Ipv4Addr::new(203, 0, 113, last)))
}

async fn make_user(db: &TestDb, login: &str, role: Role) -> (User, Actor) {
    let user = UserService::new(db.shared_store())
        .operator()
        .create(NewAccount {
            login: login.into(),
            email: format!("{login}@example.com"),
            display_name: format!("User {login}"),
            role,
            password: Some("correct horse battery".into()),
            must_reset: false,
        })
        .await
        .unwrap();
    let actor = Actor::new(user.id, user.role);
    (user, actor)
}

fn registration(redirect: &str) -> ClientRegistration {
    ClientRegistration {
        client_name: Some("Test Assistant".into()),
        redirect_uris: vec![redirect.into()],
        client_uri: None,
        logo_uri: None,
        token_endpoint_auth_method: Some("none".into()),
        grant_types: Some(vec!["authorization_code".into(), "refresh_token".into()]),
        response_types: Some(vec!["code".into()]),
    }
}

async fn registered(e: &Env) -> OAuthClient {
    e.oauth
        .register_client(registration(REDIRECT), ip(1))
        .await
        .unwrap()
}

fn params(client: &str, redirect: &str) -> AuthorizeParams {
    AuthorizeParams {
        response_type: Some("code".into()),
        client_id: Some(client.into()),
        redirect_uri: Some(redirect.into()),
        code_challenge: Some(CHALLENGE.into()),
        code_challenge_method: Some("S256".into()),
        state: Some("st-1".into()),
        scope: None,
        resource: None,
    }
}

fn query(url: &Url, key: &str) -> Option<String> {
    url.query_pairs()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
}

fn code_request(client: &str, code: &str) -> TokenRequest {
    TokenRequest {
        grant_type: Some("authorization_code".into()),
        code: Some(code.into()),
        redirect_uri: Some(REDIRECT.into()),
        client_id: Some(client.into()),
        code_verifier: Some(VERIFIER.into()),
        ..TokenRequest::default()
    }
}

fn refresh_request(client: &str, refresh: &TokenResponse) -> TokenRequest {
    TokenRequest {
        grant_type: Some("refresh_token".into()),
        refresh_token: Some(refresh.refresh_token.expose().into()),
        client_id: Some(client.into()),
        ..TokenRequest::default()
    }
}

/// Begin, approve `scopes` (all grantable when `None`) and return the code.
async fn approve_code(e: &Env, actor: &Actor, client: &str, scopes: Option<Vec<&str>>) -> String {
    let id = e.oauth.begin(params(client, REDIRECT)).await.unwrap();
    let scopes = match scopes {
        Some(s) => s.into_iter().map(String::from).collect(),
        None => e.oauth.describe(actor, id).await.unwrap().grantable,
    };
    let to = e.oauth.approve(actor, id, scopes).await.unwrap();
    query(&to, "code").expect("code")
}

async fn connect(e: &Env, actor: &Actor, client: &str) -> TokenResponse {
    let code = approve_code(e, actor, client, None).await;
    e.oauth
        .exchange(code_request(client, &code), ip(1))
        .await
        .unwrap()
}

fn invalid_grant<T: std::fmt::Debug>(r: Result<T, OAuthError>) {
    match r {
        Err(e) => assert_eq!(e.as_code(), "invalid_grant", "{e:?}"),
        Ok(v) => panic!("expected invalid_grant, got {v:?}"),
    }
}

async fn resolves(e: &Env, token: &TokenResponse) -> bool {
    e.auth
        .resolve_token(token.access_token.expose(), None)
        .await
        .unwrap()
        .is_some()
}

// ---- metadata documents ------------------------------------------------------------

async fn metadata_document_must_name_itself(db: &TestDb) {
    let e = env(db);
    e.fetcher.serve(
        DOC_URL,
        json!({
            "client_id": "https://app.example/oauth/other.json",
            "client_name": "Imposter",
            "redirect_uris": [REDIRECT],
        }),
    );
    let refused = e.oauth.begin(params(DOC_URL, REDIRECT)).await;
    assert!(
        matches!(refused, Err(AuthorizeError::ShowPage(_))),
        "{refused:?}"
    );
    assert!(
        db.store()
            .read()
            .oauth()
            .get_client(&OAuthClientId::new(DOC_URL))
            .await
            .unwrap()
            .is_none()
    );

    // Without `redirect_uris` or `client_name` it is refused too.
    e.fetcher.serve(
        DOC_URL,
        json!({ "client_id": DOC_URL, "client_name": "App" }),
    );
    assert!(matches!(
        e.oauth.begin(params(DOC_URL, REDIRECT)).await,
        Err(AuthorizeError::ShowPage(_))
    ));
    e.fetcher.serve(
        DOC_URL,
        json!({ "client_id": DOC_URL, "redirect_uris": [REDIRECT] }),
    );
    assert!(matches!(
        e.oauth.begin(params(DOC_URL, REDIRECT)).await,
        Err(AuthorizeError::ShowPage(_))
    ));

    // A document that names itself is a verified client.
    e.fetcher.serve(
        DOC_URL,
        json!({ "client_id": DOC_URL, "client_name": "App", "redirect_uris": [REDIRECT] }),
    );
    let id = e.oauth.begin(params(DOC_URL, REDIRECT)).await.unwrap();
    let (_, actor) = make_user(db, "meta", Role::Editor).await;
    let view = e.oauth.describe(&actor, id).await.unwrap();
    assert!(view.verified);
    assert_eq!(view.client.kind, ClientKind::Metadata);
    assert_eq!(view.client.name, "App");
    assert_eq!(view.redirect_host, "127.0.0.1");
}

async fn metadata_document_cached_24h_and_kept_7d_on_failure(db: &TestDb) {
    let e = env(db);
    e.fetcher.serve(
        DOC_URL,
        json!({ "client_id": DOC_URL, "client_name": "App", "redirect_uris": [REDIRECT] }),
    );
    e.oauth.begin(params(DOC_URL, REDIRECT)).await.unwrap();
    assert_eq!(e.fetcher.calls(), 1);
    // Within 24 hours the cached copy answers.
    e.clock.advance(23 * HOUR);
    e.oauth.begin(params(DOC_URL, REDIRECT)).await.unwrap();
    assert_eq!(e.fetcher.calls(), 1);
    // After 24 hours it is fetched again; a changed document replaces the copy.
    e.clock.advance(2 * HOUR);
    e.fetcher.serve(
        DOC_URL,
        json!({ "client_id": DOC_URL, "client_name": "App 2", "redirect_uris": [REDIRECT] }),
    );
    e.oauth.begin(params(DOC_URL, REDIRECT)).await.unwrap();
    assert_eq!(e.fetcher.calls(), 2);
    let stored = db
        .store()
        .read()
        .oauth()
        .get_client(&OAuthClientId::new(DOC_URL))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.name, "App 2");

    // A failing fetch keeps the copy for up to 7 days after the last fetch.
    e.fetcher.fail(DOC_URL);
    e.clock.advance(6 * 24 * HOUR);
    e.oauth.begin(params(DOC_URL, REDIRECT)).await.unwrap();
    assert_eq!(e.fetcher.calls(), 3);
    e.clock.advance(2 * 24 * HOUR);
    assert!(matches!(
        e.oauth.begin(params(DOC_URL, REDIRECT)).await,
        Err(AuthorizeError::ShowPage(_))
    ));
    assert_eq!(e.fetcher.calls(), 4);
}

// ---- authorization requests ----------------------------------------------------------

async fn loopback_redirect_port_may_differ(db: &TestDb) {
    let e = env(db);
    let client = registered(&e).await;
    let other_port = "http://127.0.0.1:50999/callback";
    let id = e
        .oauth
        .begin(params(client.id.as_str(), other_port))
        .await
        .unwrap();
    let (_, actor) = make_user(db, "port", Role::Author).await;
    let to = e
        .oauth
        .approve(&actor, id, vec!["content:read".into()])
        .await
        .unwrap();
    assert_eq!(to.port(), Some(50999));
    assert_eq!(to.path(), "/callback");
    // Another path or host on the loopback is not the registered URI.
    for bad in [
        "http://127.0.0.1:50999/other",
        "http://localhost:33418/callback",
    ] {
        assert!(matches!(
            e.oauth.begin(params(client.id.as_str(), bad)).await,
            Err(AuthorizeError::ShowPage(_))
        ));
    }
}

async fn redirect_mismatch_is_a_page_not_a_redirect(db: &TestDb) {
    let e = env(db);
    let client = registered(&e).await;
    for bad in [
        "https://evil.example/callback",
        "http://127.0.0.1:33418/callback#x",
        "",
    ] {
        let r = e.oauth.begin(params(client.id.as_str(), bad)).await;
        assert!(
            matches!(r, Err(AuthorizeError::ShowPage(_))),
            "{bad}: {r:?}"
        );
    }
    // An unknown client is a page too.
    assert!(matches!(
        e.oauth
            .begin(params("ferc_unknownunknownunknow", REDIRECT))
            .await,
        Err(AuthorizeError::ShowPage(_))
    ));
    // Once client and URI are valid, other errors go back to the client with state + iss.
    let mut p = params(client.id.as_str(), REDIRECT);
    p.code_challenge_method = Some("plain".into());
    match e.oauth.begin(p).await {
        Err(AuthorizeError::Redirect {
            uri, error, state, ..
        }) => {
            assert_eq!(error, "invalid_request");
            assert_eq!(state.as_deref(), Some("st-1"));
            assert!(uri.as_str().starts_with(REDIRECT), "{uri}");
            assert_eq!(query(&uri, "error").as_deref(), Some("invalid_request"));
            assert_eq!(query(&uri, "state").as_deref(), Some("st-1"));
            assert_eq!(query(&uri, "iss").as_deref(), Some(SITE));
        }
        other => panic!("expected a redirect, got {other:?}"),
    }
    let mut p = params(client.id.as_str(), REDIRECT);
    p.scope = Some("content:read nope:write".into());
    assert!(matches!(
        e.oauth.begin(p).await,
        Err(AuthorizeError::Redirect {
            error: "invalid_scope",
            ..
        })
    ));
}

async fn resource_trailing_slash_ok_other_refused(db: &TestDb) {
    let e = env(db);
    let client = registered(&e).await;
    for ok in [format!("{SITE}/mcp"), format!("{SITE}/mcp/")] {
        let mut p = params(client.id.as_str(), REDIRECT);
        p.resource = Some(ok);
        e.oauth.begin(p).await.unwrap();
    }
    for bad in [
        format!("{SITE}/api/v1"),
        "https://other.example/mcp".to_owned(),
        format!("{SITE}/mcp//"),
    ] {
        let mut p = params(client.id.as_str(), REDIRECT);
        p.resource = Some(bad.clone());
        assert!(
            matches!(
                e.oauth.begin(p).await,
                Err(AuthorizeError::Redirect {
                    error: "invalid_target",
                    ..
                })
            ),
            "{bad}"
        );
    }
    // At the token endpoint a wrong resource is refused as well.
    let (_, actor) = make_user(db, "res", Role::Editor).await;
    let code = approve_code(&e, &actor, client.id.as_str(), None).await;
    let mut req = code_request(client.id.as_str(), &code);
    req.resource = Some("https://other.example/mcp".into());
    let err = e.oauth.exchange(req, ip(1)).await.unwrap_err();
    assert_eq!(err.as_code(), "invalid_target");
}

// ---- codes and grants ----------------------------------------------------------------

async fn code_single_use_and_reuse_revokes_grant(db: &TestDb) {
    let e = env(db);
    let client = registered(&e).await;
    let (_, actor) = make_user(db, "code", Role::Editor).await;
    let code = approve_code(&e, &actor, client.id.as_str(), None).await;
    let first = e
        .oauth
        .exchange(code_request(client.id.as_str(), &code), ip(1))
        .await
        .unwrap();
    assert_eq!(first.token_type, "Bearer");
    assert_eq!(first.expires_in, 3600);
    assert!(resolves(&e, &first).await);
    assert_eq!(e.oauth.grants(&actor, None).await.unwrap().len(), 1);

    // The second use fails and revokes the grant the first one made.
    invalid_grant(
        e.oauth
            .exchange(code_request(client.id.as_str(), &code), ip(1))
            .await,
    );
    assert!(!resolves(&e, &first).await);
    assert!(e.oauth.grants(&actor, None).await.unwrap().is_empty());
    invalid_grant(
        e.oauth
            .exchange(refresh_request(client.id.as_str(), &first), ip(1))
            .await,
    );

    // A wrong verifier burns the code: the right one no longer works.
    let code = approve_code(&e, &actor, client.id.as_str(), None).await;
    let mut wrong = code_request(client.id.as_str(), &code);
    wrong.code_verifier = Some("x".repeat(43));
    invalid_grant(e.oauth.exchange(wrong, ip(1)).await);
    invalid_grant(
        e.oauth
            .exchange(code_request(client.id.as_str(), &code), ip(1))
            .await,
    );

    // A code expires after 60 seconds.
    let code = approve_code(&e, &actor, client.id.as_str(), None).await;
    e.clock.advance(Duration::from_secs(61));
    invalid_grant(
        e.oauth
            .exchange(code_request(client.id.as_str(), &code), ip(1))
            .await,
    );

    // Approval is audited with the client's name and the scopes, never the code.
    let rows = db
        .store()
        .read()
        .audit()
        .list(&AuditQuery {
            action: Some("oauth.connect".into()),
            ..AuditQuery::first(10)
        })
        .await
        .unwrap();
    assert_eq!(rows.items.len(), 3);
    let details = &rows.items[0].details;
    assert_eq!(details["client"], "Test Assistant");
    assert!(details["scopes"].is_array());
    assert!(!details.to_string().contains(&code));
}

async fn refresh_rotates_and_replay_revokes(db: &TestDb) {
    let e = env(db);
    let client = registered(&e).await;
    let (_, actor) = make_user(db, "refresh", Role::Editor).await;
    let first = connect(&e, &actor, client.id.as_str()).await;
    let second = e
        .oauth
        .exchange(refresh_request(client.id.as_str(), &first), ip(1))
        .await
        .unwrap();
    assert_ne!(second.refresh_token, first.refresh_token);
    // Older access tokens of the grant are deleted; the new one works.
    assert!(!resolves(&e, &first).await);
    assert!(resolves(&e, &second).await);
    // Another client cannot use the refresh token.
    invalid_grant(
        e.oauth
            .exchange(
                refresh_request("ferc_someoneelse0000000000", &second),
                ip(1),
            )
            .await,
    );
    assert!(resolves(&e, &second).await);

    // Replaying the rotated-away token revokes the grant.
    invalid_grant(
        e.oauth
            .exchange(refresh_request(client.id.as_str(), &first), ip(1))
            .await,
    );
    assert!(!resolves(&e, &second).await);
    invalid_grant(
        e.oauth
            .exchange(refresh_request(client.id.as_str(), &second), ip(1))
            .await,
    );

    // A grant ends 30 days after its last use.
    let third = connect(&e, &actor, client.id.as_str()).await;
    e.clock.advance(29 * 24 * HOUR);
    let fourth = e
        .oauth
        .exchange(refresh_request(client.id.as_str(), &third), ip(1))
        .await
        .unwrap();
    e.clock.advance(29 * 24 * HOUR);
    let fifth = e
        .oauth
        .exchange(refresh_request(client.id.as_str(), &fourth), ip(1))
        .await
        .unwrap();
    e.clock.advance(31 * 24 * HOUR);
    invalid_grant(
        e.oauth
            .exchange(refresh_request(client.id.as_str(), &fifth), ip(1))
            .await,
    );
}

async fn scope_narrowing_on_refresh_never_widens(db: &TestDb) {
    let e = env(db);
    let client = registered(&e).await;
    let (_, actor) = make_user(db, "narrow", Role::Editor).await;
    let code = approve_code(
        &e,
        &actor,
        client.id.as_str(),
        Some(vec!["content:write", "media:read"]),
    )
    .await;
    let first = e
        .oauth
        .exchange(code_request(client.id.as_str(), &code), ip(1))
        .await
        .unwrap();
    assert_eq!(first.scope, "content:write media:read");

    let mut narrow = refresh_request(client.id.as_str(), &first);
    narrow.scope = Some("content:read".into());
    let second = e.oauth.exchange(narrow, ip(1)).await.unwrap();
    assert_eq!(second.scope, "content:read");
    let who = e
        .auth
        .resolve_token(second.access_token.expose(), None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        who.actor.scopes,
        Scopes::from_strs(&["content:read".to_owned()]).unwrap()
    );

    for wider in ["menus:read", "media:write", "content:write nope:read"] {
        let mut req = refresh_request(client.id.as_str(), &second);
        req.scope = Some(wider.into());
        let err = e.oauth.exchange(req, ip(1)).await.unwrap_err();
        assert_eq!(err.as_code(), "invalid_scope", "{wider}");
    }
    // The grant keeps what the user approved: a later refresh without `scope` has it all.
    let third = e
        .oauth
        .exchange(refresh_request(client.id.as_str(), &second), ip(1))
        .await
        .unwrap();
    assert_eq!(third.scope, "content:write media:read");
}

async fn contributor_cannot_grant_beyond_role(db: &TestDb) {
    let e = env(db);
    let client = registered(&e).await;
    let (_, actor) = make_user(db, "contrib", Role::Contributor).await;
    // No `scope`: every scope is requested, the consent view offers the role's.
    let id = e
        .oauth
        .begin(params(client.id.as_str(), REDIRECT))
        .await
        .unwrap();
    let view = e.oauth.describe(&actor, id).await.unwrap();
    assert_eq!(view.requested.len(), 22);
    assert!(!view.verified);
    assert!(view.grantable.contains(&"content:write".to_owned()));
    for beyond in [
        "media:write",
        "comments:read",
        "users:write",
        "settings:write",
        "system:read",
    ] {
        assert!(!view.grantable.contains(&beyond.to_owned()), "{beyond}");
        let refused = e.oauth.approve(&actor, id, vec![beyond.into()]).await;
        assert!(matches!(refused, Err(CoreError::Forbidden(_))), "{beyond}");
    }
    // Asking for `media:write` offers `media:read`, which the role can use.
    let mut p = params(client.id.as_str(), REDIRECT);
    p.scope = Some("media:write system:write".into());
    let narrow = e.oauth.begin(p).await.unwrap();
    let view = e.oauth.describe(&actor, narrow).await.unwrap();
    assert_eq!(view.requested, ["media:write", "system:write"]);
    assert_eq!(view.grantable, ["media:read"]);
    // Scopes not requested cannot be approved either.
    assert!(matches!(
        e.oauth
            .approve(&actor, narrow, vec!["content:write".into()])
            .await,
        Err(CoreError::Forbidden(_))
    ));
    // Nothing chosen is refused; the request survives a refused approval.
    assert!(matches!(
        e.oauth.approve(&actor, id, vec![]).await,
        Err(CoreError::InvalidField { .. } | CoreError::Invalid(_))
    ));
    let to = e
        .oauth
        .approve(&actor, id, view_scopes(&e, &actor, id).await)
        .await
        .unwrap();
    assert!(query(&to, "code").is_some());
    // A request is single use.
    assert!(matches!(
        e.oauth
            .approve(&actor, id, vec!["content:read".into()])
            .await,
        Err(CoreError::NotFound)
    ));
    // Deny needs what approval needs (`users:write`); a refused deny keeps the request.
    let mut read_only = actor;
    read_only.scopes = Scopes::from_strs(&["users:read".to_owned()]).unwrap();
    assert!(matches!(
        e.oauth.deny(&read_only, narrow).await,
        Err(CoreError::Forbidden(_))
    ));
    // Deny answers access_denied with the state.
    let denied = e.oauth.deny(&actor, narrow).await.unwrap();
    assert_eq!(query(&denied, "error").as_deref(), Some("access_denied"));
    assert_eq!(query(&denied, "state").as_deref(), Some("st-1"));
    assert_eq!(query(&denied, "iss").as_deref(), Some(SITE));
}

async fn view_scopes(e: &Env, actor: &Actor, id: ferrite_domain::OAuthRequestId) -> Vec<String> {
    e.oauth.describe(actor, id).await.unwrap().grantable
}

async fn disconnect_kills_access_tokens(db: &TestDb) {
    let e = env(db);
    let client = registered(&e).await;
    let (_, actor) = make_user(db, "disc", Role::Editor).await;
    let (_, other) = make_user(db, "other", Role::Editor).await;
    let (_, admin) = make_user(db, "boss", Role::Admin).await;
    let tokens = connect(&e, &actor, client.id.as_str()).await;
    // Connecting the same client again replaces the earlier grant.
    let again = connect(&e, &actor, client.id.as_str()).await;
    assert!(!resolves(&e, &tokens).await);
    let grants = e.oauth.grants(&actor, None).await.unwrap();
    assert_eq!(grants.len(), 1);
    let grant = grants[0].grant.id;
    assert_eq!(grants[0].client.id, client.id);

    // Someone else cannot see or disconnect it; an administrator can see it.
    assert!(matches!(
        e.oauth.grants(&other, Some(actor.user)).await,
        Err(CoreError::Forbidden(_))
    ));
    assert!(matches!(
        e.oauth.disconnect(&other, grant).await,
        Err(CoreError::NotFound)
    ));
    assert_eq!(
        e.oauth
            .grants(&admin, Some(actor.user))
            .await
            .unwrap()
            .len(),
        1
    );

    e.oauth.disconnect(&actor, grant).await.unwrap();
    assert!(!resolves(&e, &again).await);
    invalid_grant(
        e.oauth
            .exchange(refresh_request(client.id.as_str(), &again), ip(1))
            .await,
    );
    assert!(matches!(
        e.oauth.disconnect(&actor, grant).await,
        Err(CoreError::NotFound)
    ));
    let rows = db
        .store()
        .read()
        .audit()
        .list(&AuditQuery {
            action: Some("oauth.disconnect".into()),
            ..AuditQuery::first(10)
        })
        .await
        .unwrap();
    assert_eq!(rows.items.len(), 1);
    assert_eq!(rows.items[0].details["client"], "Test Assistant");

    // An administrator disconnects another user's grant; RFC 7009 revocation of a refresh
    // token ends a grant too, and unknown tokens are no error.
    let third = connect(&e, &actor, client.id.as_str()).await;
    let grant = e.oauth.grants(&actor, None).await.unwrap()[0].grant.id;
    e.oauth.disconnect(&admin, grant).await.unwrap();
    assert!(!resolves(&e, &third).await);
    let fourth = connect(&e, &actor, client.id.as_str()).await;
    e.oauth.revoke(fourth.refresh_token.expose()).await.unwrap();
    assert!(!resolves(&e, &fourth).await);
    e.oauth.revoke("ferr_unknown").await.unwrap();
    e.oauth.revoke("").await.unwrap();
    // Revoking an access token ends only that token.
    let fifth = connect(&e, &actor, client.id.as_str()).await;
    e.oauth.revoke(fifth.access_token.expose()).await.unwrap();
    assert!(!resolves(&e, &fifth).await);
    assert_eq!(e.oauth.grants(&actor, None).await.unwrap().len(), 1);
}

// ---- grant-bound tokens ----------------------------------------------------------------

async fn resolve_token_marks_oauth_tokens(db: &TestDb) {
    let e = env(db);
    let client = registered(&e).await;
    let (user, actor) = make_user(db, "marks", Role::Author).await;
    let tokens = connect(&e, &actor, client.id.as_str()).await;
    assert!(tokens.access_token.expose().starts_with("fer_"));
    let who = e
        .auth
        .resolve_token(tokens.access_token.expose(), None)
        .await
        .unwrap()
        .unwrap();
    let grant = e.oauth.grants(&actor, None).await.unwrap()[0].grant.id;
    assert_eq!(who.oauth, Some(grant));
    assert_eq!(who.user.id, user.id);
    let Via::Token(token_id) = who.via else {
        panic!("expected a token")
    };
    let row = db
        .store()
        .read()
        .api_tokens()
        .get_by_hash(&sha256_hex(tokens.access_token.expose()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.id, token_id);
    assert_eq!(row.name, "Test Assistant");
    assert_eq!(row.oauth_grant_id, Some(grant));

    // A personal token is not marked.
    let (bearer, _) = e
        .auth
        .create_token(
            &actor,
            user.id,
            "cli",
            Scopes::ALL.within(ferrite_core::role_scopes(Role::Author)),
            None,
        )
        .await
        .unwrap();
    let who = e
        .auth
        .resolve_token(bearer.expose(), None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(who.oauth, None);

    // The access token lasts an hour.
    e.clock.advance(HOUR);
    assert!(!resolves(&e, &tokens).await);
}

fn sha256_hex(s: &str) -> String {
    use sha2::{Digest, Sha256};
    ferrite_domain::to_hex(&Sha256::digest(s.as_bytes()))
}

async fn personal_token_limit_ignores_oauth_tokens(db: &TestDb) {
    let e = env(db);
    let (user, actor) = make_user(db, "limit", Role::Editor).await;
    // A few connected clients' tokens neither count nor list as personal tokens.
    for n in 0..3 {
        let client = e
            .oauth
            .register_client(
                registration(&format!("http://127.0.0.1:{}/cb", 40000 + n)),
                ip(2),
            )
            .await
            .unwrap();
        let id = e
            .oauth
            .begin(AuthorizeParams {
                redirect_uri: Some(format!("http://127.0.0.1:{}/cb", 40000 + n)),
                ..params(client.id.as_str(), REDIRECT)
            })
            .await
            .unwrap();
        let to = e
            .oauth
            .approve(&actor, id, vec!["content:read".into()])
            .await
            .unwrap();
        let code = query(&to, "code").unwrap();
        e.oauth
            .exchange(
                TokenRequest {
                    redirect_uri: Some(format!("http://127.0.0.1:{}/cb", 40000 + n)),
                    ..code_request(client.id.as_str(), &code)
                },
                ip(1),
            )
            .await
            .unwrap();
    }
    for n in 0..50 {
        e.auth
            .create_token(
                &actor,
                user.id,
                &format!("t{n}"),
                Scopes::from_strs(&["content:read".to_owned()]).unwrap(),
                None,
            )
            .await
            .unwrap();
    }
    assert_eq!(e.auth.list_tokens(&actor, user.id).await.unwrap().len(), 50);
    assert!(matches!(
        e.auth
            .create_token(
                &actor,
                user.id,
                "one too many",
                Scopes::from_strs(&["content:read".to_owned()]).unwrap(),
                None,
            )
            .await,
        Err(CoreError::Invalid(_))
    ));
}

// ---- registration and limits ---------------------------------------------------------

async fn registration_is_public_only_and_limited(db: &TestDb) {
    let e = env(db);
    let client = registered(&e).await;
    assert!(client.id.as_str().starts_with("ferc_"));
    assert_eq!(client.id.as_str().len(), "ferc_".len() + 22);
    assert_eq!(client.kind, ClientKind::Registered);

    let mut secret = registration(REDIRECT);
    secret.token_endpoint_auth_method = Some("client_secret_basic".into());
    let err = e.oauth.register_client(secret, ip(9)).await.unwrap_err();
    assert_eq!(err.as_code(), "invalid_client_metadata");
    let mut grants = registration(REDIRECT);
    grants.grant_types = Some(vec!["client_credentials".into()]);
    assert_eq!(
        e.oauth
            .register_client(grants, ip(9))
            .await
            .unwrap_err()
            .as_code(),
        "invalid_client_metadata"
    );
    for bad in [
        "javascript:alert(1)",
        "data:text/html,x",
        "file:///etc/passwd",
        "http://example.com/cb",
        "https://app.example/cb#frag",
    ] {
        let err = e
            .oauth
            .register_client(registration(bad), ip(8))
            .await
            .unwrap_err();
        assert_eq!(err.as_code(), "invalid_redirect_uri", "{bad}");
    }
    for ok in [
        "cursor://anysphere.cursor-retrieval/oauth/callback",
        "http://localhost/cb",
        "http://[::1]:8080/cb",
        "https://app.example/cb?x=1",
    ] {
        e.oauth
            .register_client(registration(ok), ip(8))
            .await
            .unwrap();
    }
    // 10 per IP and hour, refused ones included.
    for _ in 0..8 {
        e.oauth
            .register_client(registration(REDIRECT), ip(9))
            .await
            .unwrap();
    }
    let limited = e
        .oauth
        .register_client(registration(REDIRECT), ip(9))
        .await
        .unwrap_err();
    assert!(
        matches!(limited, OAuthError::RateLimited { .. }),
        "{limited:?}"
    );

    // The redirect URI keeps its own query parameters.
    let with_query = e
        .oauth
        .register_client(registration("https://app.example/cb?x=1"), ip(3))
        .await
        .unwrap();
    let (_, actor) = make_user(db, "q", Role::Editor).await;
    let id = e
        .oauth
        .begin(params(with_query.id.as_str(), "https://app.example/cb?x=1"))
        .await
        .unwrap();
    let to = e
        .oauth
        .approve(&actor, id, vec!["content:read".into()])
        .await
        .unwrap();
    assert_eq!(query(&to, "x").as_deref(), Some("1"));
    assert_eq!(query(&to, "iss").as_deref(), Some(SITE));
    assert_eq!(e.oauth.issuer(), SITE);
    assert_eq!(e.oauth.resource(), format!("{SITE}/mcp"));
}

backend_tests!(
    metadata_document_must_name_itself,
    metadata_document_cached_24h_and_kept_7d_on_failure,
    loopback_redirect_port_may_differ,
    redirect_mismatch_is_a_page_not_a_redirect,
    resource_trailing_slash_ok_other_refused,
    code_single_use_and_reuse_revokes_grant,
    refresh_rotates_and_replay_revokes,
    scope_narrowing_on_refresh_never_widens,
    contributor_cannot_grant_beyond_role,
    disconnect_kills_access_tokens,
    resolve_token_marks_oauth_tokens,
    personal_token_limit_ignores_oauth_tokens,
    registration_is_public_only_and_limited,
);
