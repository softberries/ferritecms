//! AuthService on both backends (07 §2, P5–P9): login, sessions, CSRF tokens, API tokens,
//! TOTP and the DB-backed rate limits, with a controllable clock.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    net::{IpAddr, Ipv4Addr},
    sync::{Arc, Mutex},
    time::Duration,
};

use argon2::{Algorithm, Argon2, Params, PasswordHasher, Version};
use ferrite_core::{
    Actor, CoreError, NewAccount, Scopes, UserPatch, UserService,
    auth::{AuthService, IssuedSession, LoginAttempt, LoginOutcome, SiteKey, TotpSetup, Via, open},
};
use ferrite_domain::{Role, Secret, Timestamp, User, UserStatus};
use ferrite_store::Store;
use ferrite_store_tests::{TestDb, backend_tests};

const PASSWORD: &str = "correct horse battery";
const KEY: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
const MINUTE: Duration = Duration::from_secs(60);
const DAY: i64 = 24 * 3600 * 1_000_000;

/// A test clock in microseconds, shared by every service built from it.
#[derive(Clone)]
struct Clock(Arc<Mutex<i64>>);

impl Clock {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(Timestamp::now().as_micros())))
    }

    fn now(&self) -> Timestamp {
        Timestamp::from_micros(*self.0.lock().unwrap()).unwrap()
    }

    fn advance_micros(&self, micros: i64) {
        *self.0.lock().unwrap() += micros;
    }

    fn advance(&self, d: Duration) {
        self.advance_micros(i64::try_from(d.as_micros()).unwrap());
    }

    fn secs(&self) -> u64 {
        u64::try_from(self.now().as_micros() / 1_000_000).unwrap()
    }
}

fn key() -> SiteKey {
    SiteKey::from_hex(KEY).unwrap()
}

fn auth_on(store: Arc<dyn Store>, clock: &Clock) -> AuthService {
    let c = clock.clone();
    AuthService::new(store, key()).with_clock(Arc::new(move || c.now()))
}

fn ip(last: u8) -> Option<IpAddr> {
    Some(IpAddr::V4(Ipv4Addr::new(203, 0, 113, last)))
}

fn attempt<'a>(login: &'a str, password: &'a str, ip: Option<IpAddr>) -> LoginAttempt<'a> {
    LoginAttempt {
        login,
        password,
        ip,
        user_agent: None,
    }
}

async fn make_user(db: &TestDb, login: &str, role: Role) -> User {
    UserService::new(db.shared_store())
        .operator()
        .create(NewAccount {
            login: login.into(),
            email: format!("{login}@example.com"),
            display_name: format!("User {login}"),
            role,
            password: Some(PASSWORD.into()),
            must_reset: false,
        })
        .await
        .unwrap()
}

async fn session(auth: &AuthService, login: &str) -> IssuedSession {
    match auth.login(attempt(login, PASSWORD, None)).await.unwrap() {
        LoginOutcome::Session(s) => *s,
        other => panic!("expected a session, got {other:?}"),
    }
}

async fn stored(store: &dyn Store, user: &User) -> User {
    store.read().users().get(user.id).await.unwrap().unwrap()
}

fn unauthenticated<T: std::fmt::Debug>(r: Result<T, CoreError>) -> bool {
    matches!(r, Err(CoreError::Unauthenticated))
}

fn rate_limited<T: std::fmt::Debug>(r: Result<T, CoreError>) -> bool {
    match r {
        Err(CoreError::RateLimited { retry_after_secs }) => {
            assert!((1..=60).contains(&retry_after_secs), "{retry_after_secs}");
            true
        }
        other => panic!("expected RateLimited, got {other:?}"),
    }
}

async fn audit_actions(store: &dyn Store) -> Vec<String> {
    let page = store.read().audit().list(200, None).await.unwrap();
    page.items.into_iter().map(|a| a.action).collect()
}

/// Sleeps until 50 ms past the next `window` boundary of the database clock when fewer
/// than `need` remain in the current window, so a test's hits share one window (F10).
async fn align_to_window(store: &dyn Store, window: Duration, need: Duration) {
    let w = i64::try_from(window.as_micros()).unwrap();
    let now = store.read().nodes().db_now().await.unwrap().as_micros();
    let left = w - now.rem_euclid(w);
    if left < i64::try_from(need.as_micros()).unwrap() {
        tokio::time::sleep(Duration::from_micros(left.unsigned_abs()) + Duration::from_millis(50))
            .await;
    }
}

/// A `Totp` for generating codes from a [`TotpSetup`].
fn authenticator(setup: &TotpSetup) -> totp_rs::Totp {
    let secret = totp_rs::Secret::try_from_base32(&setup.secret_base32).unwrap();
    totp_rs::Builder::new().with_secret(secret).build().unwrap()
}

async fn login_issues_session_and_resolves(db: &TestDb) {
    let clock = Clock::new();
    let auth = auth_on(db.shared_store(), &clock);
    let root = make_user(db, "root", Role::Admin).await;

    let ua = "é".repeat(300);
    let outcome = auth
        .login(LoginAttempt {
            login: " ROOT ",
            password: PASSWORD,
            ip: ip(1),
            user_agent: Some(&ua),
        })
        .await
        .unwrap();
    let LoginOutcome::Session(issued) = outcome else {
        panic!("expected a session, got {outcome:?}");
    };
    assert_eq!(issued.user.id, root.id);
    let token = issued.token.expose().to_owned();
    assert_eq!(token.len(), 43, "32 bytes base64url without padding");
    assert_ne!(issued.session.id_hash, token);
    assert_eq!(issued.session.id_hash.len(), 64);
    assert_eq!(issued.session.ip_hash, Some(auth.ip_hash(ip(1).unwrap())));
    assert_eq!(
        issued
            .session
            .user_agent
            .as_deref()
            .map(|u| u.chars().count()),
        Some(256)
    );
    assert_eq!(issued.session.created_at, clock.now());
    assert_eq!(
        issued.session.expires_at.as_micros() - issued.session.created_at.as_micros(),
        30 * DAY
    );
    let row = db
        .store()
        .read()
        .sessions()
        .get(&issued.session.id_hash)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row, issued.session);
    assert_eq!(
        stored(db.store(), &root).await.last_login_at,
        Some(clock.now())
    );

    let resolved = auth.resolve_session(&token).await.unwrap().unwrap();
    assert_eq!(resolved.actor, Actor::new(root.id, Role::Admin));
    assert_eq!(resolved.actor.scopes, Scopes::ALL);
    assert_eq!(resolved.user.id, root.id);
    assert!(matches!(&resolved.via, Via::Session(s) if s.id_hash == issued.session.id_hash));

    // Login by e-mail, any ASCII case.
    let by_mail = auth
        .login(attempt("ROOT@Example.com", PASSWORD, None))
        .await
        .unwrap();
    assert!(matches!(by_mail, LoginOutcome::Session(_)));

    // Unknown and malformed tokens resolve to nothing.
    assert!(auth.resolve_session("nope").await.unwrap().is_none());
    assert!(auth.resolve_session("").await.unwrap().is_none());

    auth.logout(&token).await.unwrap();
    assert!(auth.resolve_session(&token).await.unwrap().is_none());
    // Logging out twice is fine.
    auth.logout(&token).await.unwrap();

    // issue_session (F12) issues a working session without a password.
    let fresh = auth
        .issue_session(&root, ip(2), Some("curl"))
        .await
        .unwrap();
    let resolved = auth
        .resolve_session(fresh.token.expose())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resolved.user.id, root.id);
    assert_eq!(fresh.session.user_agent.as_deref(), Some("curl"));

    // A disabled user's session resolves to nothing (status set on the row directly;
    // the service path also deletes the sessions).
    let mut row = stored(db.store(), &root).await;
    row.status = UserStatus::Disabled;
    db.store().read().users().update(&row).await.unwrap();
    assert!(
        auth.resolve_session(fresh.token.expose())
            .await
            .unwrap()
            .is_none()
    );
}

async fn unknown_user_wrong_password_and_disabled_look_the_same(db: &TestDb) {
    let clock = Clock::new();
    let auth = auth_on(db.shared_store(), &clock);
    let root = make_user(db, "root", Role::Admin).await;
    let off = make_user(db, "off", Role::Author).await;
    UserService::new(db.shared_store())
        .update(
            &Actor::new(root.id, root.role),
            off.id,
            UserPatch {
                status: Some(UserStatus::Disabled),
                ..UserPatch::default()
            },
        )
        .await
        .unwrap();
    let nopass = UserService::new(db.shared_store())
        .operator()
        .create(NewAccount {
            login: "nopass".into(),
            email: "nopass@example.com".into(),
            display_name: "No pass".into(),
            role: Role::Author,
            password: None,
            must_reset: false,
        })
        .await
        .unwrap();

    let long = format!("{}@example.com", "a".repeat(250));
    for (login, password) in [
        ("ghost", PASSWORD),
        ("ghost@example.com", PASSWORD),
        ("root", "wrong password!"),
        ("root@example.com", "wrong password!"),
        ("off", PASSWORD),
        (nopass.login.as_str(), PASSWORD),
        ("bad login!", PASSWORD),
        (long.as_str(), PASSWORD),
        ("root", ""),
    ] {
        let r = auth.login(attempt(login, password, None)).await;
        assert!(unauthenticated(r), "{login:?}");
    }
    // A failed login does not touch last_login_at.
    assert_eq!(stored(db.store(), &root).await.last_login_at, None);
}

async fn login_rate_limit_spans_nodes(db: &TestDb) {
    let clock = Clock::new();
    let peer = db.peer().await;
    let a = auth_on(db.shared_store(), &clock);
    let b = auth_on(peer.clone().unwrap_or_else(|| db.shared_store()), &clock);
    for login in ["ann", "ben", "cat", "dan"] {
        make_user(db, login, Role::Author).await;
    }
    align_to_window(db.store(), MINUTE, Duration::from_secs(40)).await;

    // Ten failed attempts from one IP, alternating nodes, spread over three logins so
    // the per-login limit (5/min) is never reached (F9).
    let logins = [
        "ann", "ann", "ann", "ann", "ben", "ben", "ben", "ben", "cat", "cat",
    ];
    for (i, login) in logins.iter().enumerate() {
        let node = if i % 2 == 0 { &a } else { &b };
        let r = node.login(attempt(login, "wrong password!", ip(1))).await;
        assert!(unauthenticated(r), "attempt {}", i + 1);
    }
    // The 11th from that IP is refused even with the right password, on either node.
    assert!(rate_limited(b.login(attempt("dan", PASSWORD, ip(1))).await));
    assert!(rate_limited(a.login(attempt("dan", PASSWORD, ip(1))).await));

    // Another IP, same login: "ann" has 4 hits; the 5th passes to verification, the 6th
    // is refused even with the right password.
    assert!(unauthenticated(
        a.login(attempt("ann", "wrong password!", ip(2))).await
    ));
    assert!(rate_limited(b.login(attempt("ANN", PASSWORD, ip(3))).await));
    // Other logins from fresh IPs still work.
    assert!(matches!(
        a.login(attempt("dan", PASSWORD, ip(4))).await.unwrap(),
        LoginOutcome::Session(_)
    ));
}

async fn session_idle_and_absolute_expiry(db: &TestDb) {
    let clock = Clock::new();
    let auth = auth_on(db.shared_store(), &clock);
    make_user(db, "root", Role::Admin).await;

    // Idle: 7 d + 1 s after the last touch.
    let idle = session(&auth, "root").await;
    clock.advance_micros(7 * DAY + 1_000_000);
    assert!(
        auth.resolve_session(idle.token.expose())
            .await
            .unwrap()
            .is_none()
    );
    // Expired rows are deleted on sight.
    assert!(
        db.store()
            .read()
            .sessions()
            .get(&idle.session.id_hash)
            .await
            .unwrap()
            .is_none()
    );

    // Touched every 6 days, the session lives until 30 days after creation.
    let active = session(&auth, "root").await;
    let token = active.token.expose();
    for _ in 0..4 {
        clock.advance_micros(6 * DAY);
        assert!(auth.resolve_session(token).await.unwrap().is_some());
    }
    clock.advance_micros(6 * DAY - 1_000_000); // 30 d - 1 s after creation
    assert!(auth.resolve_session(token).await.unwrap().is_some());
    clock.advance_micros(1_000_000); // exactly 30 d
    assert!(auth.resolve_session(token).await.unwrap().is_none());
}

async fn touch_is_throttled(db: &TestDb) {
    let clock = Clock::new();
    let auth = auth_on(db.shared_store(), &clock);
    make_user(db, "root", Role::Admin).await;
    let issued = session(&auth, "root").await;
    let last_seen = || async {
        db.store()
            .read()
            .sessions()
            .get(&issued.session.id_hash)
            .await
            .unwrap()
            .unwrap()
            .last_seen_at
    };

    // Within 5 min of the last write: no touch.
    clock.advance(Duration::from_secs(4 * 60));
    auth.resolve_session(issued.token.expose())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(last_seen().await, issued.session.created_at);

    clock.advance(Duration::from_secs(2 * 60));
    let touched_at = clock.now();
    auth.resolve_session(issued.token.expose())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(last_seen().await, touched_at);

    clock.advance(MINUTE);
    auth.resolve_session(issued.token.expose())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(last_seen().await, touched_at, "a second write within 5 min");
}

async fn revocation_is_immediate_on_peer(db: &TestDb) {
    let clock = Clock::new();
    let peer = db.peer().await.unwrap_or_else(|| db.shared_store());
    let a = auth_on(db.shared_store(), &clock);
    let root = make_user(db, "root", Role::Admin).await;
    let alice = make_user(db, "alice", Role::Author).await;
    let issued = session(&a, "alice").await;
    let (bearer, _) = a
        .create_token(
            &Actor::new(alice.id, alice.role),
            alice.id,
            "ci",
            Scopes::ALL,
            None,
        )
        .await
        .unwrap();
    assert!(
        a.resolve_session(issued.token.expose())
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        a.resolve_token(bearer.expose(), None)
            .await
            .unwrap()
            .is_some()
    );

    // Password change on the other node: the old session is gone at once on this one.
    UserService::new(peer.clone())
        .set_password(
            &Actor::new(alice.id, alice.role),
            alice.id,
            Some(PASSWORD),
            "another long password",
            None,
        )
        .await
        .unwrap();
    assert!(
        a.resolve_session(issued.token.expose())
            .await
            .unwrap()
            .is_none()
    );

    // Disable on the other node: the token stops working at once.
    UserService::new(peer)
        .update(
            &Actor::new(root.id, root.role),
            alice.id,
            UserPatch {
                status: Some(UserStatus::Disabled),
                ..UserPatch::default()
            },
        )
        .await
        .unwrap();
    assert!(
        a.resolve_token(bearer.expose(), None)
            .await
            .unwrap()
            .is_none()
    );
}

async fn totp_enroll_login_and_replay(db: &TestDb) {
    let clock = Clock::new();
    let auth = auth_on(db.shared_store(), &clock).with_issuer_name("My: Blog".into());
    let user = make_user(db, "tia", Role::Author).await;
    let actor = Actor::new(user.id, user.role);

    let setup = auth.totp_setup(&actor).await.unwrap();
    assert!(
        setup.otpauth_url.starts_with("otpauth://totp/"),
        "{}",
        setup.otpauth_url
    );
    assert!(setup.otpauth_url.contains("tia"), "{}", setup.otpauth_url);
    assert!(setup.otpauth_url.contains("Blog"), "{}", setup.otpauth_url);
    assert!(!setup.otpauth_url.contains("My:"), "{}", setup.otpauth_url);
    assert!(setup.enrollment.starts_with("v1."));
    let totp = authenticator(&setup);

    // A wrong code, or an enrollment of someone else, does not enable TOTP.
    let wrong = format!(
        "{:06}",
        (totp
            .generate(clock.secs())
            .to_string()
            .parse::<u32>()
            .unwrap()
            + 1)
            % 1_000_000
    );
    assert!(matches!(
        auth.totp_confirm(&actor, None, &setup.enrollment, &wrong, PASSWORD)
            .await,
        Err(CoreError::Invalid(_))
    ));
    let other = make_user(db, "oz", Role::Author).await;
    let code = totp.generate(clock.secs()).to_string();
    assert!(matches!(
        auth.totp_confirm(
            &Actor::new(other.id, other.role),
            None,
            &setup.enrollment,
            &code,
            PASSWORD
        )
        .await,
        Err(CoreError::Invalid(_))
    ));
    // The right code without the account's password does not enable it either.
    assert!(matches!(
        auth.totp_confirm(&actor, None, &setup.enrollment, &code, "not the password")
            .await,
        Err(CoreError::Forbidden("current password"))
    ));
    assert!(stored(db.store(), &user).await.totp_secret.is_none());

    auth.totp_confirm(&actor, None, &setup.enrollment, &code, PASSWORD)
        .await
        .unwrap();
    let row = stored(db.store(), &user).await;
    let sealed = row.totp_secret.clone().unwrap();
    assert!(sealed.expose().starts_with("v1."));
    let raw = open(&key(), &format!("totp-secret:{}", user.id), sealed.expose()).unwrap();
    assert_eq!(raw, totp.secret().as_ref());
    assert!(open(&key(), "totp-secret", sealed.expose()).is_err());
    assert!(
        open(
            &key(),
            &format!("totp-secret:{}", other.id),
            sealed.expose()
        )
        .is_err()
    );
    // Setup while TOTP is on is refused (F11).
    assert!(matches!(
        auth.totp_setup(&actor).await,
        Err(CoreError::Invalid(m)) if m == "disable TOTP first"
    ));

    // The confirm code's step is used up; the next step logs in. Logins go by e-mail
    // here: the confirm's password re-checks used the login's own bucket (R11).
    clock.advance(Duration::from_secs(30));
    let LoginOutcome::TotpRequired { challenge } = auth
        .login(attempt("tia@example.com", PASSWORD, None))
        .await
        .unwrap()
    else {
        panic!("expected a TOTP challenge");
    };
    let code = totp.generate(clock.secs()).to_string();
    let issued = auth
        .login_totp(&challenge, &code, ip(9), None)
        .await
        .unwrap();
    assert_eq!(issued.user.id, user.id);
    assert!(
        auth.resolve_session(issued.token.expose())
            .await
            .unwrap()
            .is_some()
    );

    // The same code again (fresh challenge) is a replay.
    let LoginOutcome::TotpRequired { challenge } = auth
        .login(attempt("tia@example.com", PASSWORD, None))
        .await
        .unwrap()
    else {
        panic!("expected a TOTP challenge");
    };
    assert!(unauthenticated(
        auth.login_totp(&challenge, &code, None, None).await
    ));
    // A forged challenge is refused.
    assert!(unauthenticated(
        auth.login_totp("v1.AAAA", &code, None, None).await
    ));

    // The same challenge, expired (5 min), is refused even with a fresh code.
    clock.advance(Duration::from_secs(6 * 60));
    let code = totp.generate(clock.secs()).to_string();
    assert!(unauthenticated(
        auth.login_totp(&challenge, &code, None, None).await
    ));

    // Disabling needs the password.
    assert!(matches!(
        auth.totp_disable(&actor, "wrong password!").await,
        Err(CoreError::Forbidden(_))
    ));
    auth.totp_disable(&actor, PASSWORD).await.unwrap();
    assert!(stored(db.store(), &user).await.totp_secret.is_none());
    assert!(matches!(
        auth.login(attempt("tia@example.com", PASSWORD, None))
            .await
            .unwrap(),
        LoginOutcome::Session(_)
    ));
    let actions = audit_actions(db.store()).await;
    assert!(actions.contains(&"totp.enable".to_owned()), "{actions:?}");
    assert!(actions.contains(&"totp.disable".to_owned()), "{actions:?}");
}

/// Enrols `user` through setup + confirm and returns its authenticator.
async fn enrol(auth: &AuthService, clock: &Clock, user: &User) -> totp_rs::Totp {
    let actor = Actor::new(user.id, user.role);
    let setup = auth.totp_setup(&actor).await.unwrap();
    let totp = authenticator(&setup);
    let code = totp.generate(clock.secs()).to_string();
    auth.totp_confirm(&actor, None, &setup.enrollment, &code, PASSWORD)
        .await
        .unwrap();
    totp
}

/// A code that is not valid around `secs` (± one step).
fn wrong_code(totp: &totp_rs::Totp, secs: u64) -> String {
    let valid: Vec<String> = [secs - 30, secs, secs + 30]
        .iter()
        .map(|&t| totp.generate(t).to_string())
        .collect();
    (0..1_000_000u32)
        .map(|n| format!("{n:06}"))
        .find(|c| !valid.contains(c))
        .unwrap()
}

async fn challenge(auth: &AuthService, login: &str, password: &str) -> String {
    match auth.login(attempt(login, password, None)).await.unwrap() {
        LoginOutcome::TotpRequired { challenge } => challenge,
        other => panic!("expected a TOTP challenge, got {other:?}"),
    }
}

async fn totp_login_needs_both_factors(db: &TestDb) {
    let clock = Clock::new();
    let auth = auth_on(db.shared_store(), &clock);
    let user = make_user(db, "lee", Role::Author).await;
    let totp = enrol(&auth, &clock, &user).await;
    clock.advance(Duration::from_secs(30));

    // Password right, second factor missing, wrong or expired: not a login (R12).
    let first = challenge(&auth, "lee", PASSWORD).await;
    let outcome = auth.login(attempt("lee", PASSWORD, None)).await.unwrap();
    assert!(
        !format!("{outcome:?}").contains("v1."),
        "challenge redacted"
    );
    assert_eq!(stored(db.store(), &user).await.last_login_at, None);
    let wrong = wrong_code(&totp, clock.secs());
    assert!(unauthenticated(
        auth.login_totp(&first, &wrong, None, None).await
    ));
    assert_eq!(stored(db.store(), &user).await.last_login_at, None);
    clock.advance(Duration::from_secs(6 * 60));
    let code = totp.generate(clock.secs()).to_string();
    assert!(unauthenticated(
        auth.login_totp(&first, &code, None, None).await
    ));
    assert_eq!(stored(db.store(), &user).await.last_login_at, None);

    // Both factors: last_login_at is set.
    let second = challenge(&auth, "lee", PASSWORD).await;
    auth.login_totp(&second, &code, None, None).await.unwrap();
    assert_eq!(
        stored(db.store(), &user).await.last_login_at,
        Some(clock.now())
    );

    // A password change invalidates outstanding challenges, and the refused code is
    // not used up.
    clock.advance(Duration::from_secs(30));
    let stale = challenge(&auth, "lee", PASSWORD).await;
    UserService::new(db.shared_store())
        .operator()
        .set_password("lee", "a brand new password", false)
        .await
        .unwrap();
    let code = totp.generate(clock.secs()).to_string();
    assert!(unauthenticated(
        auth.login_totp(&stale, &code, None, None).await
    ));
    // By e-mail: the confirm's password re-check and five logins used the login's own
    // bucket (5/min).
    let fresh = challenge(&auth, "lee@example.com", "a brand new password").await;
    auth.login_totp(&fresh, &code, None, None).await.unwrap();
}

async fn totp_attempts_are_rate_limited(db: &TestDb) {
    let clock = Clock::new();
    let auth = auth_on(db.shared_store(), &clock);
    let ann = make_user(db, "ann", Role::Author).await;
    let ben = make_user(db, "ben", Role::Author).await;
    align_to_window(db.store(), MINUTE, Duration::from_secs(30)).await;

    // totp_confirm: 5 attempts a minute (each also re-checks the password on the login
    // bucket); the 6th is refused even when right.
    let ann_actor = Actor::new(ann.id, ann.role);
    let setup = auth.totp_setup(&ann_actor).await.unwrap();
    let totp = authenticator(&setup);
    let wrong = wrong_code(&totp, clock.secs());
    for i in 0..5 {
        assert!(
            matches!(
                auth.totp_confirm(&ann_actor, None, &setup.enrollment, &wrong, PASSWORD)
                    .await,
                Err(CoreError::Invalid(_))
            ),
            "attempt {}",
            i + 1
        );
    }
    let code = totp.generate(clock.secs()).to_string();
    assert!(rate_limited(
        auth.totp_confirm(&ann_actor, None, &setup.enrollment, &code, PASSWORD)
            .await
    ));
    assert!(stored(db.store(), &ann).await.totp_secret.is_none());

    // login_totp: the confirm used one hit; 4 wrong codes, then the right one is refused.
    let totp = enrol(&auth, &clock, &ben).await;
    clock.advance(Duration::from_secs(30));
    let ch = challenge(&auth, "ben", PASSWORD).await;
    let wrong = wrong_code(&totp, clock.secs());
    for i in 0..4 {
        assert!(
            unauthenticated(auth.login_totp(&ch, &wrong, None, None).await),
            "attempt {}",
            i + 1
        );
    }
    let code = totp.generate(clock.secs()).to_string();
    assert!(rate_limited(auth.login_totp(&ch, &code, None, None).await));
    assert_eq!(stored(db.store(), &ben).await.last_login_at, None);

    // totp_disable's password re-check shares the login bucket: the confirm's re-check
    // and one login so far, 3 wrong passwords, then the right one is refused.
    let ben_actor = Actor::new(ben.id, ben.role);
    for i in 0..3 {
        assert!(
            matches!(
                auth.totp_disable(&ben_actor, "wrong password!").await,
                Err(CoreError::Forbidden(_))
            ),
            "attempt {}",
            i + 1
        );
    }
    assert!(rate_limited(auth.totp_disable(&ben_actor, PASSWORD).await));
    assert!(stored(db.store(), &ben).await.totp_secret.is_some());
}

async fn csrf_token_is_bound_to_the_session(db: &TestDb) {
    let clock = Clock::new();
    let auth = auth_on(db.shared_store(), &clock);
    make_user(db, "root", Role::Admin).await;
    let one = session(&auth, "root").await.session;
    let two = session(&auth, "root").await.session;

    let token = auth.csrf_token(&one);
    assert_eq!(token.len(), 43);
    assert_eq!(token, auth.csrf_token(&one), "stable per session");
    assert!(auth.check_csrf(&one, &token));
    assert!(!auth.check_csrf(&two, &token));
    assert!(auth.check_csrf(&two, &auth.csrf_token(&two)));
    let mut bytes = token.clone().into_bytes();
    bytes[0] = if bytes[0] == b'A' { b'B' } else { b'A' };
    let modified = String::from_utf8(bytes).unwrap();
    assert!(!auth.check_csrf(&one, &modified));
    assert!(!auth.check_csrf(&one, &token[..42]));
    assert!(!auth.check_csrf(&one, ""));
    assert!(!auth.check_csrf(&one, &format!("{token}A")));
}

async fn tokens_scoped_counted_and_rate_limited(db: &TestDb) {
    let clock = Clock::new();
    let auth = auth_on(db.shared_store(), &clock);
    let root = make_user(db, "root", Role::Admin).await;
    let alice = make_user(db, "alice", Role::Author).await;
    let bob = make_user(db, "bob", Role::Editor).await;
    let (admin, me, editor) = (
        Actor::new(root.id, root.role),
        Actor::new(alice.id, alice.role),
        Actor::new(bob.id, bob.role),
    );
    let read_only = Scopes::from_strs(&["content:read".into()]).unwrap();

    let (bearer, token) = auth
        .create_token(&me, alice.id, "  deploy  ", read_only, None)
        .await
        .unwrap();
    assert!(bearer.expose().starts_with("fer_"));
    assert_eq!(bearer.expose().len(), 4 + 43);
    assert_eq!(token.name, "deploy");
    assert_eq!(token.scopes, vec!["content:read".to_owned()]);
    assert_eq!(token.token_hash.len(), 64);
    assert!(!bearer.expose().contains(&token.token_hash));
    let resolved = auth
        .resolve_token(bearer.expose(), ip(1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        resolved.actor,
        Actor::new(alice.id, Role::Author).with_scopes(read_only)
    );
    assert!(matches!(resolved.via, Via::Token(id) if id == token.id));

    // A narrow token cannot mint tokens.
    assert!(matches!(
        auth.create_token(&resolved.actor, alice.id, "x", Scopes::ALL, None)
            .await,
        Err(CoreError::Forbidden(_))
    ));
    // Validation.
    for (name, scopes) in [
        ("", read_only),
        (&*"n".repeat(101), read_only),
        ("ok", Scopes::NONE),
    ] {
        assert!(
            matches!(
                auth.create_token(&me, alice.id, name, scopes, None).await,
                Err(CoreError::Invalid(_))
            ),
            "{name:?}"
        );
    }
    assert!(matches!(
        auth.create_token(&me, alice.id, "past", read_only, Some(clock.now()))
            .await,
        Err(CoreError::Invalid(_))
    ));
    // Another user's tokens need manage_users.
    assert!(matches!(
        auth.create_token(&editor, alice.id, "x", read_only, None)
            .await,
        Err(CoreError::Forbidden(_))
    ));
    assert!(matches!(
        auth.list_tokens(&editor, alice.id).await,
        Err(CoreError::Forbidden(_))
    ));
    assert!(matches!(
        auth.revoke_token(&editor, alice.id, token.id).await,
        Err(CoreError::Forbidden(_))
    ));
    assert_eq!(auth.list_tokens(&me, alice.id).await.unwrap().len(), 1);
    assert_eq!(auth.list_tokens(&admin, alice.id).await.unwrap().len(), 1);

    // At most 50 per user.
    for i in 1..50 {
        auth.create_token(&admin, alice.id, &format!("t{i}"), read_only, None)
            .await
            .unwrap();
    }
    assert!(matches!(
        auth.create_token(&me, alice.id, "t51", read_only, None)
            .await,
        Err(CoreError::Invalid(_))
    ));
    assert_eq!(auth.list_tokens(&me, alice.id).await.unwrap().len(), 50);

    // Revoke.
    auth.revoke_token(&me, alice.id, token.id).await.unwrap();
    assert!(
        auth.resolve_token(bearer.expose(), None)
            .await
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        auth.revoke_token(&me, alice.id, token.id).await,
        Err(CoreError::NotFound)
    ));
    let actions = audit_actions(db.store()).await;
    assert!(actions.contains(&"token.create".to_owned()), "{actions:?}");
    assert!(actions.contains(&"token.revoke".to_owned()), "{actions:?}");

    // Expiry.
    let one_hour = Timestamp::from_micros(clock.now().as_micros() + 3_600_000_000).unwrap();
    let (bob_bearer, _) = auth
        .create_token(&editor, bob.id, "short", Scopes::ALL, Some(one_hour))
        .await
        .unwrap();
    assert!(
        auth.resolve_token(bob_bearer.expose(), None)
            .await
            .unwrap()
            .is_some()
    );
    clock.advance(Duration::from_secs(2 * 3600));
    assert!(
        auth.resolve_token(bob_bearer.expose(), None)
            .await
            .unwrap()
            .is_none()
    );

    // A disabled owner's token resolves to nothing (row status set directly; the
    // service path also deletes the tokens).
    let (root_bearer, _) = auth
        .create_token(&admin, root.id, "admin", Scopes::ALL, None)
        .await
        .unwrap();
    let mut row = stored(db.store(), &root).await;
    row.status = UserStatus::Disabled;
    db.store().read().users().update(&row).await.unwrap();
    assert!(
        auth.resolve_token(root_bearer.expose(), None)
            .await
            .unwrap()
            .is_none()
    );

    // Unknown bearer tokens: 10 per minute per IP.
    align_to_window(db.store(), MINUTE, Duration::from_secs(30)).await;
    for i in 0..10 {
        let r = auth.resolve_token(&format!("fer_unknown{i}"), ip(7)).await;
        assert!(r.unwrap().is_none(), "attempt {}", i + 1);
    }
    assert!(rate_limited(auth.resolve_token("fer_unknown", ip(7)).await));
    // Another IP is not limited.
    assert!(
        auth.resolve_token("fer_unknown", ip(8))
            .await
            .unwrap()
            .is_none()
    );
}

async fn rehash_on_login_when_params_change(db: &TestDb) {
    let clock = Clock::new();
    let auth = auth_on(db.shared_store(), &clock);
    let user = make_user(db, "old", Role::Author).await;
    let weak = Argon2::new(
        Algorithm::Argon2id,
        Version::V0x13,
        Params::new(Params::DEFAULT_M_COST, 1, 1, None).unwrap(),
    )
    .hash_password(PASSWORD.as_bytes())
    .unwrap()
    .to_string();
    assert!(weak.contains("t=1"), "{weak}");
    let mut row = stored(db.store(), &user).await;
    row.password_hash = Some(Secret::new(weak));
    db.store().read().users().update(&row).await.unwrap();

    session(&auth, "old").await;
    let hash = stored(db.store(), &user).await.password_hash.unwrap();
    assert!(
        hash.expose().starts_with("$argon2id$v=19$m=19456,t=2,p=1$"),
        "{}",
        hash.expose()
    );
    // The new hash still logs in.
    session(&auth, "old").await;
}

/// R26 (M2): enabling TOTP ends the user's other sessions, so a stolen cookie stops
/// working; the session that confirmed is kept. Confirmed through a token (no session
/// to keep), every session ends. Other users' sessions are untouched.
async fn totp_confirm_ends_other_sessions(db: &TestDb) {
    let clock = Clock::new();
    let auth = auth_on(db.shared_store(), &clock);
    let session_of = |o: LoginOutcome| match o {
        LoginOutcome::Session(s) => s,
        _ => panic!("expected a session"),
    };
    let mia = make_user(db, "mia", Role::Author).await;
    let bystander = make_user(db, "bo", Role::Author).await;
    let mine = session_of(auth.login(attempt("mia", PASSWORD, None)).await.unwrap());
    let stolen = session_of(
        auth.login(attempt("mia@example.com", PASSWORD, None))
            .await
            .unwrap(),
    );
    let theirs = session_of(auth.login(attempt("bo", PASSWORD, None)).await.unwrap());

    let actor = Actor::new(mia.id, mia.role);
    let setup = auth.totp_setup(&actor).await.unwrap();
    let code = authenticator(&setup).generate(clock.secs()).to_string();
    auth.totp_confirm(
        &actor,
        Some(&mine.session.id_hash),
        &setup.enrollment,
        &code,
        PASSWORD,
    )
    .await
    .unwrap();
    let alive = |s: &IssuedSession| {
        let token = s.token.expose().to_owned();
        let auth = &auth;
        async move { auth.resolve_session(&token).await.unwrap().is_some() }
    };
    assert!(alive(&mine).await, "the confirming session is kept");
    assert!(!alive(&stolen).await, "other sessions end");
    assert!(alive(&theirs).await, "other users are untouched");

    // Through a token there is no session to keep: all of them end.
    let other = session_of(
        auth.login(attempt("bo@example.com", PASSWORD, None))
            .await
            .unwrap(),
    );
    let bo = Actor::new(bystander.id, bystander.role);
    let setup = auth.totp_setup(&bo).await.unwrap();
    let code = authenticator(&setup).generate(clock.secs()).to_string();
    auth.totp_confirm(&bo, None, &setup.enrollment, &code, PASSWORD)
        .await
        .unwrap();
    assert!(!alive(&theirs).await);
    assert!(!alive(&other).await);
    assert!(alive(&mine).await);
}

/// `hit_limit` (Task 8, P16): one count per scope and client (/64 for IPv6), shared by
/// every node, a window of the caller's choice, `Retry-After` within that window.
async fn hit_limit_counts_per_scope_and_client(db: &TestDb) {
    let clock = Clock::new();
    let peer = db.peer().await;
    let a = auth_on(db.shared_store(), &clock);
    let b = auth_on(peer.clone().unwrap_or_else(|| db.shared_store()), &clock);
    let window = Duration::from_secs(5);
    align_to_window(db.store(), window, Duration::from_secs(3)).await;
    for i in 0..3 {
        let node = if i % 2 == 0 { &a } else { &b };
        node.hit_limit("route:x:1", ip(1), 3, window).await.unwrap();
    }
    match a.hit_limit("route:x:1", ip(1), 3, window).await {
        Err(CoreError::RateLimited { retry_after_secs }) => {
            assert!((1..=5).contains(&retry_after_secs), "{retry_after_secs}");
        }
        other => panic!("expected RateLimited, got {other:?}"),
    }
    // Another client, another scope, and no address at all are separate buckets.
    b.hit_limit("route:x:1", ip(2), 3, window).await.unwrap();
    b.hit_limit("route:x:2", ip(1), 3, window).await.unwrap();
    for _ in 0..3 {
        a.hit_limit("route:x:1", None, 3, window).await.unwrap();
    }
    assert!(a.hit_limit("route:x:1", None, 3, window).await.is_err());
    // The per-plugin client hash: keyed, bounded, per plugin, not the session hash.
    let v4: IpAddr = "198.51.100.1".parse().unwrap();
    let mapped = IpAddr::V6(Ipv4Addr::new(198, 51, 100, 1).to_ipv6_mapped());
    let h = a.plugin_ip_hash("router", v4);
    assert_eq!(h.len(), 32);
    assert!(
        h.bytes()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    );
    assert_eq!(h, b.plugin_ip_hash("router", mapped));
    assert_ne!(h, a.plugin_ip_hash("route-xss", v4));
    assert_ne!(h, a.ip_hash(v4));
    assert_ne!(
        h,
        a.plugin_ip_hash("router", "198.51.100.2".parse().unwrap())
    );
    // `plugin || 0x00 || ip`: no name/address split collides.
    assert_ne!(
        a.plugin_ip_hash("a1", "1.2.3.4".parse().unwrap()),
        a.plugin_ip_hash("a", "11.2.3.4".parse().unwrap())
    );
    // One IPv6 /64 is one client.
    let v6 = |s: &str| Some(s.parse::<IpAddr>().unwrap());
    a.hit_limit("route:y", v6("2001:db8::1"), 1, window)
        .await
        .unwrap();
    assert!(
        b.hit_limit("route:y", v6("2001:db8::ffff"), 1, window)
            .await
            .is_err()
    );
}

backend_tests!(
    login_issues_session_and_resolves,
    unknown_user_wrong_password_and_disabled_look_the_same,
    login_rate_limit_spans_nodes,
    session_idle_and_absolute_expiry,
    touch_is_throttled,
    revocation_is_immediate_on_peer,
    totp_enroll_login_and_replay,
    totp_confirm_ends_other_sessions,
    totp_login_needs_both_factors,
    totp_attempts_are_rate_limited,
    csrf_token_is_bound_to_the_session,
    tokens_scoped_counted_and_rate_limited,
    rehash_on_login_when_params_change,
    hit_limit_counts_per_scope_and_client,
);
