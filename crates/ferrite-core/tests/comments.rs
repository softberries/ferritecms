//! CommentService on both backends: submit rules, moderation, events, the honeypot,
//! rate limits, delete and threads (M2a-2b P11–P14, R2, F18–F22).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    net::{IpAddr, Ipv4Addr},
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration,
};

use ferrite_core::{
    Actor, AuthService, ContentDraft, ContentService, CoreError, PermalinkRules, Scopes,
    UserService,
    auth::SiteKey,
    comments::{
        COMMENT_HOUR_SCOPE, COMMENTS_ENABLED_DEFAULT, COMMENTS_MODERATION_DEFAULT, CommentInput,
        CommentService, Commenter, MAX_COMMENT_BODY, Submitted, render_body,
    },
    events::post_key,
};
use ferrite_domain::{
    Comment, CommentId, CommentQuery, CommentStatus, ContentId, ContentItem, NewUser, Role,
    Timestamp, UserId, UserStatus,
};
use ferrite_store::Store;
use ferrite_store_tests::{TestDb, backend_tests};
use serde_json::json;

struct Site {
    store: Arc<dyn Store>,
    content: ContentService,
    comments: CommentService,
    auth: Arc<AuthService>,
    admin: Actor,
}

async fn site(db: &TestDb) -> Site {
    let store = db.shared_store();
    let auth = Arc::new(AuthService::new(
        store.clone(),
        SiteKey::generate().unwrap(),
    ));
    let admin = actor(store.as_ref(), Role::Admin).await;
    Site {
        content: ContentService::new(store.clone(), Arc::new(PermalinkRules::default())),
        comments: CommentService::new(store.clone(), auth.clone()),
        store,
        auth,
        admin,
    }
}

async fn actor(store: &dyn Store, role: Role) -> Actor {
    let id = UserId::new();
    let tx = store.begin().await.unwrap();
    tx.users()
        .insert(&NewUser {
            id,
            login: format!("user-{id}"),
            email: format!("{id}@example.com"),
            display_name: format!("Name {}", &id.to_string()[..8]),
            password_hash: None,
            role,
            status: UserStatus::Active,
            must_reset: false,
        })
        .await
        .unwrap();
    tx.commit().await.unwrap();
    Actor::new(id, role)
}

fn draft(kind: &str, slug: &str) -> ContentDraft {
    ContentDraft {
        kind: kind.into(),
        title: format!("Title {slug}"),
        slug: Some(slug.into()),
        parent_id: None,
        excerpt: None,
        body: json!({"type": "doc", "content": []}),
        fields: json!({}),
        menu_order: 0,
        terms: Vec::new(),
    }
}

impl Site {
    async fn published(&self, kind: &str, slug: &str) -> ContentItem {
        let item = self
            .content
            .create(&self.admin, draft(kind, slug))
            .await
            .unwrap();
        self.content
            .publish(&self.admin, item.id, item.version)
            .await
            .unwrap()
    }

    async fn set(&self, key: &str, value: serde_json::Value) {
        self.store
            .read()
            .options()
            .set(key, &value, true)
            .await
            .unwrap();
    }

    async fn submit(&self, input: CommentInput) -> Result<Submitted, CoreError> {
        self.comments
            .submit(input, Commenter::Anonymous, next_ip())
            .await
    }

    async fn accepted(&self, input: CommentInput) -> Comment {
        match self.submit(input).await.unwrap() {
            Submitted::Accepted(c) => c,
            Submitted::Discarded => panic!("discarded"),
        }
    }

    async fn approve(&self, id: CommentId) -> Comment {
        self.comments
            .moderate(&self.admin, id, CommentStatus::Approved)
            .await
            .unwrap()
    }

    async fn count(&self) -> usize {
        self.store
            .read()
            .comments()
            .list(&CommentQuery {
                limit: 200,
                ..Default::default()
            })
            .await
            .unwrap()
            .items
            .len()
    }

    async fn max_seq(&self) -> i64 {
        self.store.read().events().max_seq().await.unwrap()
    }

    async fn keys_since(&self, seq: i64) -> Vec<String> {
        self.store
            .read()
            .events()
            .since(seq, 100)
            .await
            .unwrap()
            .into_iter()
            .flat_map(|e| e.keys)
            .collect()
    }
}

/// A fresh client address per call (the per-IP limits stay out of the way).
fn next_ip() -> IpAddr {
    static NEXT: AtomicU32 = AtomicU32::new(1);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    IpAddr::V4(Ipv4Addr::from(0x0a00_0000 | n))
}

fn input(item: ContentId) -> CommentInput {
    CommentInput {
        item,
        kind: "post".into(),
        parent: None,
        author_name: "Ann".into(),
        author_email: "ann@example.com".into(),
        author_url: String::new(),
        body: "Nice post.".into(),
        honeypot: String::new(),
    }
}

fn invalid_field<T: std::fmt::Debug>(r: Result<T, CoreError>, want: &str) {
    match r {
        Err(CoreError::InvalidField { field, .. }) => assert_eq!(field, want),
        other => panic!("expected InvalidField({want}), got {other:?}"),
    }
}

fn not_found<T: std::fmt::Debug>(r: Result<T, CoreError>) {
    assert!(matches!(r, Err(CoreError::NotFound)), "{r:?}");
}

fn forbidden<T: std::fmt::Debug>(r: Result<T, CoreError>, what: &str) {
    match r {
        Err(CoreError::Forbidden(w)) => assert_eq!(w, what),
        other => panic!("expected Forbidden({what}), got {other:?}"),
    }
}

/// Waits for the next window of `window` (database clock) unless `need` is left (F10).
async fn align_to_window(store: &dyn Store, window: Duration, need: Duration) {
    let w = i64::try_from(window.as_micros()).unwrap();
    let now = store.read().nodes().db_now().await.unwrap().as_micros();
    let left = w - now.rem_euclid(w);
    if left < i64::try_from(need.as_micros()).unwrap() {
        tokio::time::sleep(Duration::from_micros(left.unsigned_abs()) + Duration::from_millis(50))
            .await;
    }
}

async fn submit_rules(db: &TestDb) {
    let s = site(db).await;
    let post = s.published("post", "rules").await;
    let other = s.published("post", "other").await;

    // One case per validation rule; nothing is written by any of them.
    let with = |f: &dyn Fn(&mut CommentInput)| {
        let mut i = input(post.id);
        f(&mut i);
        i
    };
    let cases: Vec<(&str, CommentInput)> = vec![
        ("body", with(&|i| i.body = "   ".into())),
        ("body", with(&|i| i.body = "x".repeat(MAX_COMMENT_BODY + 1))),
        ("body", with(&|i| i.body = "bell \u{7} here".into())),
        ("body", with(&|i| i.body = "nul \0 here".into())),
        ("author_name", with(&|i| i.author_name = "  ".into())),
        ("author_name", with(&|i| i.author_name = "n".repeat(101))),
        ("author_name", with(&|i| i.author_name = "a\nb".into())),
        ("author_email", with(&|i| i.author_email = String::new())),
        ("author_email", with(&|i| i.author_email = "nope".into())),
        (
            "author_email",
            with(&|i| i.author_email = format!("{}@example.com", "a".repeat(250))),
        ),
        ("author_email", with(&|i| i.author_email = "a b@c.d".into())),
        (
            "author_url",
            with(&|i| i.author_url = "javascript:alert(1)".into()),
        ),
        (
            "author_url",
            with(&|i| i.author_url = "ftp://x.example".into()),
        ),
        ("author_url", with(&|i| i.author_url = "http://".into())),
        (
            "author_url",
            with(&|i| i.author_url = "https:///path".into()),
        ),
        ("author_url", with(&|i| i.author_url = "example.com".into())),
        (
            "author_url",
            with(&|i| i.author_url = "https://user@evil.example/".into()),
        ),
        (
            "author_url",
            with(&|i| i.author_url = "https://a.example/\"onmouseover=x".into()),
        ),
        (
            "author_url",
            with(&|i| i.author_url = format!("https://a.example/{}", "p".repeat(200))),
        ),
        ("parent_id", with(&|i| i.parent = Some(CommentId::new()))),
    ];
    for (field, case) in cases {
        invalid_field(s.submit(case).await, field);
    }
    assert_eq!(s.count().await, 0);

    // Accepted shapes: trimmed fields, CRLF folded, `\n` and `\t` kept, http(s) URLs.
    let ok = s
        .accepted(with(&|i| {
            i.author_name = "  Ann  ".into();
            i.author_email = " Ann@Example.com ".into();
            i.author_url = " HTTPS://ann.example:8443/blog?x=1#y ".into();
            i.body = "  line one\r\n\tline two\r\n\r\npara  ".into();
        }))
        .await;
    assert_eq!(ok.author_name, "Ann");
    assert_eq!(ok.author_email, "ann@example.com");
    assert_eq!(ok.author_url, "HTTPS://ann.example:8443/blog?x=1#y");
    assert_eq!(ok.body, "line one\n\tline two\n\npara");
    assert_eq!(ok.status, CommentStatus::Pending);
    assert_eq!(ok.user_id, None);
    let hash = ok.ip_hash.clone().unwrap();
    assert_eq!(hash.len(), 32);
    assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    let ipv6 = s
        .accepted(with(&|i| i.author_url = "http://[2001:db8::1]/".into()))
        .await;
    assert_eq!(ipv6.author_url, "http://[2001:db8::1]/");

    // Unknown, draft, page, kind mismatch and disabled comments: NotFound.
    not_found(s.submit(input(ContentId::new())).await);
    let drafted = s
        .content
        .create(&s.admin, draft("post", "drafted"))
        .await
        .unwrap();
    not_found(s.submit(input(drafted.id)).await);
    let page = s.published("page", "a-page").await;
    let mut on_page = input(page.id);
    not_found(s.submit(on_page.clone()).await);
    on_page.kind = "page".into();
    not_found(s.submit(on_page).await);
    let mut wrong_kind = input(post.id);
    wrong_kind.kind = "page".into();
    not_found(s.submit(wrong_kind).await);
    let mut trashed = s.published("post", "trashed").await;
    trashed = s
        .content
        .trash(&s.admin, trashed.id, trashed.version)
        .await
        .unwrap();
    not_found(s.submit(input(trashed.id)).await);

    // Replies: the parent must be an approved comment of the same item.
    let pending = s.accepted(input(post.id)).await;
    let mut reply = input(post.id);
    reply.parent = Some(pending.id);
    invalid_field(s.submit(reply.clone()).await, "parent_id");
    s.approve(pending.id).await;
    let mut elsewhere = input(other.id);
    elsewhere.parent = Some(pending.id);
    invalid_field(s.submit(elsewhere).await, "parent_id");
    // Depth 1..=5 nest; a reply at depth 6 is refused.
    let mut parent = pending.id;
    for _ in 1..=5 {
        let mut r = input(post.id);
        r.parent = Some(parent);
        let c = s.accepted(r).await;
        assert_eq!(c.parent_id, Some(parent));
        s.approve(c.id).await;
        parent = c.id;
    }
    let mut deep = input(post.id);
    deep.parent = Some(parent);
    invalid_field(s.submit(deep).await, "parent_id");

    // Disabled: NotFound for every item.
    s.set("comments.enabled", json!(false)).await;
    not_found(s.submit(input(post.id)).await);
    s.set("comments.enabled", json!(true)).await;

    // A signed-in commenter's name and e-mail come from the account.
    let user = actor(s.store.as_ref(), Role::Contributor).await;
    let account = s
        .store
        .read()
        .users()
        .get(user.user)
        .await
        .unwrap()
        .unwrap();
    let mut signed = input(post.id);
    signed.author_name = "Mallory".into();
    signed.author_email = "mallory@evil.example".into();
    let got = s
        .comments
        .submit(signed.clone(), Commenter::User(user), next_ip())
        .await
        .unwrap();
    let Submitted::Accepted(got) = got else {
        panic!("discarded")
    };
    assert_eq!(got.user_id, Some(user.user));
    assert_eq!(got.author_name, account.display_name);
    assert_eq!(got.author_email, account.email.to_ascii_lowercase());
    assert_eq!(got.status, CommentStatus::Pending);
    // A token without `comments:write` cannot comment as its user.
    let token = user.with_scopes(Scopes::from_strs(&["content:read".to_owned()]).unwrap());
    forbidden(
        s.comments
            .submit(signed, Commenter::User(token), next_ip())
            .await,
        "comments:write",
    );
}

async fn moderation_decides_status(db: &TestDb) {
    let s = site(db).await;
    let post = s.published("post", "moderated").await;

    // `all`: every guest comment is held, even from a known commenter.
    s.set("comments.moderation", json!("all")).await;
    let first = s.accepted(input(post.id)).await;
    assert_eq!(first.status, CommentStatus::Pending);
    s.approve(first.id).await;
    assert_eq!(
        s.accepted(input(post.id)).await.status,
        CommentStatus::Pending
    );
    // A moderator's own comment is approved in any mode.
    let editor = actor(s.store.as_ref(), Role::Editor).await;
    let got = s
        .comments
        .submit(input(post.id), Commenter::User(editor), next_ip())
        .await
        .unwrap();
    let Submitted::Accepted(got) = got else {
        panic!("discarded")
    };
    assert_eq!(got.status, CommentStatus::Approved);

    // `known`: the same name and e-mail with an approved comment post directly.
    s.set("comments.moderation", json!("known")).await;
    let mut known = input(post.id);
    known.author_name = " Ann ".into();
    known.author_email = "ANN@example.com".into();
    known.body = "see http://a.example and www.b.example".into();
    assert_eq!(
        s.accepted(known.clone()).await.status,
        CommentStatus::Approved
    );
    // `https://www.` is one link, so this is still two.
    known.body = "https://www.a.example/x and HTTP://b.example".into();
    assert_eq!(
        s.accepted(known.clone()).await.status,
        CommentStatus::Approved
    );
    // Three links hold it.
    known.body = "http://a.example https://b.example www.c.example".into();
    assert_eq!(s.accepted(known).await.status, CommentStatus::Pending);
    // Another name with the same e-mail, or another e-mail: held.
    let mut other_name = input(post.id);
    other_name.author_name = "Anne".into();
    assert_eq!(s.accepted(other_name).await.status, CommentStatus::Pending);
    let mut other_mail = input(post.id);
    other_mail.author_email = "ann@example.org".into();
    assert_eq!(s.accepted(other_mail).await.status, CommentStatus::Pending);

    // An account's address never vouches for a guest (F19), even with an approved guest
    // comment under that name and address from before the account existed.
    let user = actor(s.store.as_ref(), Role::Contributor).await;
    let account = s
        .store
        .read()
        .users()
        .get(user.user)
        .await
        .unwrap()
        .unwrap();
    let mut guest = input(post.id);
    guest.author_name = account.display_name.clone();
    guest.author_email = account.email.clone();
    let c = s.accepted(guest.clone()).await;
    s.approve(c.id).await;
    assert_eq!(s.accepted(guest).await.status, CommentStatus::Pending);

    // A signed-in commenter is known by their own approved comments.
    let signed = |who: Actor| {
        let comments = &s.comments;
        async move {
            match comments
                .submit(input(post.id), Commenter::User(who), next_ip())
                .await
                .unwrap()
            {
                Submitted::Accepted(c) => c,
                Submitted::Discarded => panic!("discarded"),
            }
        }
    };
    let first = signed(user).await;
    assert_eq!(first.status, CommentStatus::Pending);
    s.approve(first.id).await;
    assert_eq!(signed(user).await.status, CommentStatus::Approved);

    // A deleted account's approved comments (now without `user_id`) do not vouch for a
    // guest using its name and address (Task 1 carry).
    let gone = actor(s.store.as_ref(), Role::Contributor).await;
    let gone_user = s
        .store
        .read()
        .users()
        .get(gone.user)
        .await
        .unwrap()
        .unwrap();
    let c = signed(gone).await;
    s.approve(c.id).await;
    UserService::new(s.store.clone())
        .delete(&s.admin, gone.user, None)
        .await
        .unwrap();
    let orphan = s.store.read().comments().get(c.id).await.unwrap().unwrap();
    assert_eq!(orphan.user_id, None);
    let mut impostor = input(post.id);
    impostor.author_name = gone_user.display_name;
    impostor.author_email = gone_user.email;
    assert_eq!(s.accepted(impostor).await.status, CommentStatus::Pending);
}

/// F20: with no setting stored, moderation is `known` and comments are on.
async fn moderation_default_is_known(db: &TestDb) {
    const { assert!(COMMENTS_ENABLED_DEFAULT) };
    assert_eq!(COMMENTS_MODERATION_DEFAULT, "known");
    let s = site(db).await;
    let post = s.published("post", "defaults").await;
    let first = s.accepted(input(post.id)).await;
    assert_eq!(first.status, CommentStatus::Pending);
    s.approve(first.id).await;
    assert_eq!(
        s.accepted(input(post.id)).await.status,
        CommentStatus::Approved
    );
}

async fn approval_invalidates_the_post(db: &TestDb) {
    let s = site(db).await;
    let post = s.published("post", "events").await;
    let key = post_key(post.id);

    let before = s.max_seq().await;
    let c = s.accepted(input(post.id)).await;
    assert!(s.keys_since(before).await.is_empty(), "a pending insert");

    let before = s.max_seq().await;
    let approved = s.approve(c.id).await;
    assert_eq!(approved.status, CommentStatus::Approved);
    assert_eq!(s.keys_since(before).await, std::slice::from_ref(&key));

    let before = s.max_seq().await;
    s.comments
        .moderate(&s.admin, c.id, CommentStatus::Trash)
        .await
        .unwrap();
    assert_eq!(s.keys_since(before).await, std::slice::from_ref(&key));

    let before = s.max_seq().await;
    s.comments.delete(&s.admin, c.id).await.unwrap();
    assert_eq!(s.keys_since(before).await, std::slice::from_ref(&key));

    // An approved insert (a moderator's comment) invalidates in its own `Tx`.
    let editor = actor(s.store.as_ref(), Role::Editor).await;
    let before = s.max_seq().await;
    s.comments
        .submit(input(post.id), Commenter::User(editor), next_ip())
        .await
        .unwrap();
    assert_eq!(s.keys_since(before).await, [key]);

    // Moderation needs `moderate_comments` and `comments:write`.
    let author = actor(s.store.as_ref(), Role::Author).await;
    forbidden(
        s.comments
            .moderate(&author, c.id, CommentStatus::Approved)
            .await,
        "moderate_comments",
    );
    let read_only = s
        .admin
        .with_scopes(Scopes::from_strs(&["comments:read".to_owned()]).unwrap());
    forbidden(
        s.comments
            .moderate(&read_only, c.id, CommentStatus::Approved)
            .await,
        "comments:write",
    );
    not_found(
        s.comments
            .moderate(&s.admin, CommentId::new(), CommentStatus::Approved)
            .await,
    );

    // The queue needs a moderator holding `comments:read`.
    let all = CommentQuery {
        limit: 50,
        ..Default::default()
    };
    forbidden(
        s.comments.list(&author, all.clone()).await,
        "moderate_comments",
    );
    let no_scope = s
        .admin
        .with_scopes(Scopes::from_strs(&["content:read".to_owned()]).unwrap());
    forbidden(
        s.comments.list(&no_scope, all.clone()).await,
        "comments:read",
    );
    let page = s.comments.list(&read_only, all).await.unwrap();
    assert_eq!(page.items.len(), 1);
}

async fn honeypot_discards(db: &TestDb) {
    let s = site(db).await;
    let post = s.published("post", "honey").await;
    let before = s.max_seq().await;
    let mut bot = input(post.id);
    bot.honeypot = "http://spam.example".into();
    assert!(matches!(
        s.submit(bot.clone()).await,
        Ok(Submitted::Discarded)
    ));
    // Even an otherwise invalid one is dropped silently.
    bot.body = String::new();
    assert!(matches!(s.submit(bot).await, Ok(Submitted::Discarded)));
    assert_eq!(s.count().await, 0);
    assert!(s.keys_since(before).await.is_empty());
}

async fn comment_limits(db: &TestDb) {
    let s = site(db).await;
    let post = s.published("post", "limits").await;
    let ip = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 9));
    let submit = |ip: IpAddr| s.comments.submit(input(post.id), Commenter::Anonymous, ip);

    align_to_window(
        s.store.as_ref(),
        Duration::from_secs(60),
        Duration::from_secs(20),
    )
    .await;
    for _ in 0..3 {
        submit(ip).await.unwrap();
    }
    match submit(ip).await {
        Err(CoreError::RateLimited { retry_after_secs }) => {
            assert!((1..=60).contains(&retry_after_secs));
        }
        other => panic!("4th in a minute: {other:?}"),
    }
    // A refused submit (and a honeypot one) still counts; another address passes.
    let mut bot = input(post.id);
    bot.honeypot = "x".into();
    assert!(matches!(
        s.comments.submit(bot, Commenter::Anonymous, ip).await,
        Err(CoreError::RateLimited { .. })
    ));
    submit(IpAddr::V4(Ipv4Addr::new(198, 51, 100, 10)))
        .await
        .unwrap();

    // 20 an hour: an address with 20 hits this hour is refused its 21st.
    let busy = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 11));
    let hour = Duration::from_secs(3600);
    align_to_window(s.store.as_ref(), hour, Duration::from_secs(20)).await;
    for _ in 0..20 {
        s.auth
            .hit_limit(COMMENT_HOUR_SCOPE, Some(busy), 20, hour)
            .await
            .unwrap();
    }
    assert!(matches!(
        submit(busy).await,
        Err(CoreError::RateLimited { .. })
    ));
    assert_eq!(s.count().await, 4);
}

async fn delete_only_spam_or_trash(db: &TestDb) {
    let s = site(db).await;
    let post = s.published("post", "delete").await;
    let c = s.accepted(input(post.id)).await;

    invalid_field(s.comments.delete(&s.admin, c.id).await, "status");
    s.approve(c.id).await;
    invalid_field(s.comments.delete(&s.admin, c.id).await, "status");
    let author = actor(s.store.as_ref(), Role::Author).await;
    forbidden(s.comments.delete(&author, c.id).await, "moderate_comments");

    for status in [CommentStatus::Spam, CommentStatus::Trash] {
        let c = s.accepted(input(post.id)).await;
        s.comments.moderate(&s.admin, c.id, status).await.unwrap();
        s.comments.delete(&s.admin, c.id).await.unwrap();
        assert!(s.store.read().comments().get(c.id).await.unwrap().is_none());
    }
    not_found(s.comments.delete(&s.admin, CommentId::new()).await);
    assert_eq!(s.count().await, 1);
}

async fn thread_depths_and_cap(db: &TestDb) {
    let s = site(db).await;
    let post = s.published("post", "thread").await;

    // a (0) ── b (1) ── c (2); d (0); e replies to a trashed comment: shown at depth 0.
    let at = |micros: i64| Timestamp::from_micros(1_700_000_000_000_000 + micros).unwrap();
    let row = |parent: Option<CommentId>, n: i64, status: CommentStatus| Comment {
        id: CommentId::new(),
        item_id: post.id,
        parent_id: parent,
        user_id: None,
        author_name: format!("<b>n{n}</b>"),
        author_email: format!("n{n}@example.com"),
        author_url: if n == 0 {
            "https://a.example/".into()
        } else {
            String::new()
        },
        body: format!("body {n} <script>alert(1)</script>\n\nnext"),
        status,
        ip_hash: None,
        created_at: at(n),
    };
    let repo = s.store.read().comments();
    let a = row(None, 0, CommentStatus::Approved);
    let d = row(None, 1, CommentStatus::Approved);
    let b = row(Some(a.id), 2, CommentStatus::Approved);
    let gone = row(None, 3, CommentStatus::Trash);
    let e = row(Some(gone.id), 4, CommentStatus::Approved);
    let c = row(Some(b.id), 5, CommentStatus::Approved);
    let held = row(Some(a.id), 6, CommentStatus::Pending);
    for r in [&a, &d, &b, &gone, &e, &c, &held] {
        repo.insert(r).await.unwrap();
    }
    let thread = s.comments.thread(post.id).await.unwrap();
    let shape: Vec<(CommentId, u8, Option<CommentId>)> = thread
        .iter()
        .map(|t| (t.id, t.depth, t.parent_id))
        .collect();
    assert_eq!(
        shape,
        [
            (a.id, 0, None),
            (b.id, 1, Some(a.id)),
            (c.id, 2, Some(b.id)),
            (d.id, 0, None),
            (e.id, 0, None),
        ]
    );
    let first = &thread[0];
    assert_eq!(first.author_name, "<b>n0</b>", "stored as text");
    assert_eq!(first.author_url.as_deref(), Some("https://a.example/"));
    assert_eq!(thread[1].author_url, None);
    assert_eq!(
        first.html,
        "<p>body 0 &lt;script&gt;alert(1)&lt;/script&gt;</p><p>next</p>"
    );
    assert_eq!(first.date, a.created_at);

    // At most 500 approved comments are shown (the oldest).
    let other = s.published("post", "busy").await;
    for n in 0..501 {
        let mut r = row(None, 100 + n, CommentStatus::Approved);
        r.item_id = other.id;
        repo.insert(&r).await.unwrap();
    }
    let thread = s.comments.thread(other.id).await.unwrap();
    assert_eq!(thread.len(), 500);
    assert_eq!(thread[0].date, at(100));
    assert_eq!(thread[499].date, at(599));
}

#[test]
fn render_body_escapes_and_breaks() {
    assert_eq!(render_body("a"), "<p>a</p>");
    assert_eq!(render_body("a\nb"), "<p>a<br>b</p>");
    assert_eq!(render_body("a\n\n\n b \n\nc"), "<p>a</p><p> b </p><p>c</p>");
    assert_eq!(render_body("a\n \t\nb"), "<p>a</p><p>b</p>");
    assert_eq!(
        render_body("<img src=x onerror=alert(1)> & \"q\" 'q'"),
        "<p>&lt;img src=x onerror=alert(1)&gt; &amp; \"q\" 'q'</p>"
    );
    assert_eq!(
        render_body("see https://example.com/"),
        "<p>see https://example.com/</p>",
        "no links are made"
    );
    assert_eq!(render_body(""), "");
}

backend_tests!(
    submit_rules,
    moderation_decides_status,
    moderation_default_is_known,
    approval_invalidates_the_post,
    honeypot_discards,
    comment_limits,
    delete_only_spam_or_trash,
    thread_depths_and_cap,
);
