//! User contract tests.

use std::{sync::Arc, time::Duration};

use ferrite_domain::{
    ContentItem, NewAudit, NewUser, Revision, RevisionId, Role, Secret, Timestamp, User, UserId,
    UserStatus,
};
use ferrite_store::{Store, StoreError};
use serde_json::json;
use tokio::sync::Barrier;

use crate::{
    TestDb,
    fixtures::{fixture_user, new_post},
};

fn new_user(login: &str, email: &str) -> NewUser {
    NewUser {
        id: UserId::new(),
        login: login.into(),
        email: email.into(),
        display_name: format!("Name of {login}"),
        password_hash: Some(Secret::new("$argon2id$v=19$stub")),
        role: Role::Editor,
        status: UserStatus::Active,
        must_reset: true,
    }
}

async fn insert(store: &dyn Store, user: &NewUser) -> Result<User, StoreError> {
    store.read().users().insert(user).await
}

pub async fn users_insert_get(db: &TestDb) {
    let store = db.store();
    let new = new_user("alice", "Alice@Example.com");
    let stored = insert(store, &new).await.unwrap();
    assert_eq!(stored.id, new.id);
    assert_eq!(stored.login, "alice");
    assert_eq!(stored.email, "Alice@Example.com");
    assert_eq!(stored.password_hash, new.password_hash);
    assert_eq!(
        stored.password_hash.as_ref().unwrap().expose(),
        "$argon2id$v=19$stub"
    );
    assert_eq!(stored.role, Role::Editor);
    assert!(stored.must_reset);
    assert!(stored.totp_secret.is_none() && stored.last_login_at.is_none());

    let users = store.read().users();
    assert_eq!(users.get(new.id).await.unwrap().unwrap(), stored);
    assert_eq!(users.get_by_login("alice").await.unwrap().unwrap(), stored);
    assert_eq!(
        users
            .get_by_email("Alice@Example.com")
            .await
            .unwrap()
            .unwrap(),
        stored
    );
    assert!(users.get(UserId::new()).await.unwrap().is_none());
    assert!(users.get_by_login("nobody").await.unwrap().is_none());
    assert!(
        users
            .get_by_email("nobody@example.com")
            .await
            .unwrap()
            .is_none()
    );

    let mut no_password = new_user("bob", "bob@example.com");
    no_password.password_hash = None;
    assert!(
        insert(store, &no_password)
            .await
            .unwrap()
            .password_hash
            .is_none()
    );

    let dup_login = new_user("alice", "other@example.com");
    assert!(matches!(
        insert(store, &dup_login).await,
        Err(StoreError::UniqueViolation { .. })
    ));
}

pub async fn email_case_insensitive_unique(db: &TestDb) {
    let store = db.store();
    let first = insert(store, &new_user("first", "USER@Example.com"))
        .await
        .unwrap();
    // Stored exactly as given.
    assert_eq!(first.email, "USER@Example.com");
    assert!(matches!(
        insert(store, &new_user("second", "user@example.com")).await,
        Err(StoreError::UniqueViolation { .. })
    ));

    let users = store.read().users();
    for probe in ["User@EXAMPLE.com", "user@example.com", "USER@EXAMPLE.COM"] {
        assert_eq!(
            users.get_by_email(probe).await.unwrap().unwrap().id,
            first.id
        );
    }

    // Changing the e-mail's case on the same user is fine; colliding with another is not.
    let mut changed = first.clone();
    changed.email = "user@example.com".into();
    users.update(&changed).await.unwrap();
    assert_eq!(
        users.get(first.id).await.unwrap().unwrap().email,
        "user@example.com"
    );

    let other = insert(store, &new_user("other", "other@example.com"))
        .await
        .unwrap();
    let mut clash = other.clone();
    clash.email = "USER@example.com".into();
    assert!(matches!(
        users.update(&clash).await,
        Err(StoreError::UniqueViolation { .. })
    ));
}

pub async fn unicode_login_and_slug_exact(db: &TestDb) {
    let store = db.store();
    let user = insert(store, &new_user("Ünïcode", "unicode@example.com"))
        .await
        .unwrap();
    assert_eq!(user.login, "Ünïcode");
    let users = store.read().users();
    assert_eq!(
        users.get_by_login("Ünïcode").await.unwrap().unwrap().id,
        user.id
    );
    // Logins are exact: neither case nor normalisation differences match.
    assert!(users.get_by_login("ünïcode").await.unwrap().is_none());
    assert!(users.get_by_login("Unicode").await.unwrap().is_none());
    // And case variants of a login are distinct accounts.
    insert(store, &new_user("ünïcode", "lower@example.com"))
        .await
        .unwrap();
    insert(store, &new_user("Alice", "a1@example.com"))
        .await
        .unwrap();
    insert(store, &new_user("alice", "a2@example.com"))
        .await
        .unwrap();

    let post = store
        .read()
        .content()
        .insert(&new_post(&user, "zażółć"))
        .await
        .unwrap();
    assert_eq!(post.slug, "zażółć");
    let found = store
        .read()
        .content()
        .get_by_slug("post", None, "zażółć")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.id, post.id);
    assert!(
        store
            .read()
            .content()
            .get_by_slug("post", None, "ZAŻÓŁĆ")
            .await
            .unwrap()
            .is_none()
    );
}

pub async fn users_update_and_list(db: &TestDb) {
    let store = db.store();
    let users = store.read().users();
    for (login, email) in [
        ("carol", "c@example.com"),
        ("Zed", "z@example.com"),
        ("alice", "a@example.com"),
        ("Bob", "b@example.com"),
        ("dave", "d@example.com"),
    ] {
        insert(store, &new_user(login, email)).await.unwrap();
    }

    // Binary order: uppercase sorts before lowercase.
    let mut seen = Vec::new();
    let mut cursor: Option<String> = None;
    let mut pages = 0;
    loop {
        let page = users.list(2, cursor.as_deref()).await.unwrap();
        pages += 1;
        seen.extend(page.items.iter().map(|u| u.login.clone()));
        match page.next_cursor {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    assert_eq!(seen, ["Bob", "Zed", "alice", "carol", "dave"]);
    assert_eq!(pages, 3);
    let exact = users.list(5, None).await.unwrap();
    assert_eq!(exact.items.len(), 5);
    assert!(exact.next_cursor.is_none());

    assert!(matches!(
        users.list(0, None).await,
        Err(StoreError::Invalid(_))
    ));
    assert!(matches!(
        users.list(2, Some("not a cursor")).await,
        Err(StoreError::Invalid(_))
    ));

    // update replaces every mutable field.
    let mut alice = users.get_by_login("alice").await.unwrap().unwrap();
    alice.login = "alice2".into();
    alice.email = "alice2@example.com".into();
    alice.display_name = "Alice Two".into();
    alice.password_hash = None;
    alice.role = Role::Admin;
    alice.status = UserStatus::Disabled;
    alice.must_reset = false;
    alice.last_login_at = Some(Timestamp::now());
    users.update(&alice).await.unwrap();
    let got = users.get(alice.id).await.unwrap().unwrap();
    assert_eq!(got, alice);
    assert!(users.get_by_login("alice").await.unwrap().is_none());

    // Clearing optional fields works too.
    alice.last_login_at = None;
    users.update(&alice).await.unwrap();
    assert_eq!(users.get(alice.id).await.unwrap().unwrap(), alice);

    // Colliding login is a unique violation; a missing user is NotFound.
    let mut clash = alice.clone();
    clash.login = "carol".into();
    assert!(matches!(
        users.update(&clash).await,
        Err(StoreError::UniqueViolation { .. })
    ));
    let mut ghost = alice.clone();
    ghost.id = UserId::new();
    assert!(matches!(
        users.update(&ghost).await,
        Err(StoreError::NotFound)
    ));
}

/// R12: e-mail folding is ASCII-only and done in Rust, so it is identical on both backends.
pub async fn email_folding_is_ascii_only(db: &TestDb) {
    let store = db.store();
    let upper = insert(store, &new_user("upper", "É@x.com")).await.unwrap();
    let lower = insert(store, &new_user("lower", "é@x.com")).await.unwrap();
    assert_ne!(upper.id, lower.id);
    let users = store.read().users();
    assert_eq!(
        users.get_by_email("É@x.com").await.unwrap().unwrap().id,
        upper.id
    );
    assert_eq!(
        users.get_by_email("é@x.com").await.unwrap().unwrap().id,
        lower.id
    );
    // Only the ASCII part folds.
    assert_eq!(
        users.get_by_email("É@X.COM").await.unwrap().unwrap().id,
        upper.id
    );
    assert_eq!(
        users.get_by_email("é@X.com").await.unwrap().unwrap().id,
        lower.id
    );

    insert(store, &new_user("a", "USER@x.com")).await.unwrap();
    assert!(matches!(
        insert(store, &new_user("b", "user@x.com")).await,
        Err(StoreError::UniqueViolation { .. })
    ));
    assert_eq!(
        users
            .get_by_email("User@X.com")
            .await
            .unwrap()
            .unwrap()
            .email,
        "USER@x.com"
    );
}

fn user_with(login: &str, role: Role, status: UserStatus) -> NewUser {
    NewUser {
        role,
        status,
        ..new_user(login, &format!("{login}@example.com"))
    }
}

fn revision_by(item: &ContentItem, author: UserId) -> Revision {
    Revision {
        id: RevisionId::new(),
        item_id: item.id,
        author_id: author,
        created_at: Timestamp::now(),
        title: item.title.clone(),
        excerpt: String::new(),
        body: json!([]),
        fields: json!({}),
        meta: json!({}),
        note: None,
    }
}

async fn last_step(store: &dyn Store, id: UserId) -> Option<i64> {
    store
        .read()
        .users()
        .get(id)
        .await
        .unwrap()
        .unwrap()
        .totp_last_step
}

/// P9: a TOTP step is accepted once; replays and older steps are refused, also when two
/// nodes race for the same step.
pub async fn totp_step_only_moves_forward(db: Arc<TestDb>) {
    let store = db.shared_store();
    let user = insert(store.as_ref(), &new_user("totp", "totp@example.com"))
        .await
        .unwrap();
    assert_eq!(user.totp_last_step, None);
    let users = store.read().users();
    assert!(users.advance_totp_step(user.id, 5).await.unwrap());
    assert_eq!(last_step(store.as_ref(), user.id).await, Some(5));
    assert!(
        !users.advance_totp_step(user.id, 5).await.unwrap(),
        "replay"
    );
    assert!(!users.advance_totp_step(user.id, 4).await.unwrap(), "older");
    assert_eq!(last_step(store.as_ref(), user.id).await, Some(5));
    assert!(users.advance_totp_step(user.id, 6).await.unwrap());
    assert_eq!(last_step(store.as_ref(), user.id).await, Some(6));
    // An unknown user has no step to move.
    assert!(!users.advance_totp_step(UserId::new(), 1).await.unwrap());

    // Two nodes submit the same step at once: exactly one wins, every round.
    for step in 7..27 {
        let barrier = Arc::new(Barrier::new(2));
        let mut handles = Vec::new();
        for _ in 0..2 {
            let node: Arc<dyn Store> = db.peer().await.unwrap_or_else(|| store.clone());
            let barrier = barrier.clone();
            handles.push(tokio::spawn(async move {
                barrier.wait().await;
                node.read()
                    .users()
                    .advance_totp_step(user.id, step)
                    .await
                    .unwrap()
            }));
        }
        let mut wins = 0;
        for h in handles {
            wins += usize::from(h.await.unwrap());
        }
        assert_eq!(wins, 1, "step {step}");
    }
    assert_eq!(last_step(store.as_ref(), user.id).await, Some(26));
}

/// F4: `set_totp` replaces the secret and resets the replay guard in one statement.
pub async fn set_totp_sets_secret_and_resets_step(db: &TestDb) {
    let store = db.store();
    let users = store.read().users();
    let user = insert(store, &new_user("enrol", "enrol@example.com"))
        .await
        .unwrap();
    assert!(users.advance_totp_step(user.id, 40).await.unwrap());

    users
        .set_totp(user.id, Some(&Secret::new("sealed-secret")))
        .await
        .unwrap();
    let got = users.get(user.id).await.unwrap().unwrap();
    assert_eq!(got.totp_secret.as_ref().unwrap().expose(), "sealed-secret");
    assert_eq!(got.totp_last_step, None);
    // The guard starts over: an old step is accepted again for the new secret.
    assert!(users.advance_totp_step(user.id, 3).await.unwrap());

    users.set_totp(user.id, None).await.unwrap();
    let got = users.get(user.id).await.unwrap().unwrap();
    assert!(got.totp_secret.is_none());
    assert_eq!(got.totp_last_step, None);
    // Everything else is untouched.
    assert_eq!(
        User {
            totp_secret: None,
            totp_last_step: None,
            ..user.clone()
        },
        got
    );

    assert!(matches!(
        users.set_totp(UserId::new(), None).await,
        Err(StoreError::NotFound)
    ));
    assert!(matches!(
        users.set_totp(user.id, Some(&Secret::new("JB\0SW"))).await,
        Err(StoreError::Invalid(_))
    ));
}

/// `update` writes neither TOTP column (F4): a stale row cannot undo a replay guard or an
/// enrolment that happened after it was read.
pub async fn update_keeps_totp_last_step(db: &TestDb) {
    let store = db.store();
    let users = store.read().users();
    let user = insert(store, &new_user("stale", "stale@example.com"))
        .await
        .unwrap();
    let mut stale = users.get(user.id).await.unwrap().unwrap();
    users
        .set_totp(user.id, Some(&Secret::new("enrolled")))
        .await
        .unwrap();
    assert!(users.advance_totp_step(user.id, 11).await.unwrap());

    stale.display_name = "Renamed".into();
    stale.totp_secret = Some(Secret::new("ignored"));
    stale.totp_last_step = Some(1);
    users.update(&stale).await.unwrap();
    let got = users.get(user.id).await.unwrap().unwrap();
    assert_eq!(got.display_name, "Renamed");
    assert_eq!(got.totp_secret.as_ref().unwrap().expose(), "enrolled");
    assert_eq!(got.totp_last_step, Some(11));
    assert!(!users.advance_totp_step(user.id, 11).await.unwrap());
}

/// P16: content and revisions restrict deleting their author; reassigning both in one `Tx`
/// lets the delete through. Sessions and tokens cascade, audit rows keep a NULL actor.
pub async fn user_delete_restricted_by_content_then_ok_after_reassign(db: &TestDb) {
    let store = db.store();
    let author = fixture_user(store).await;
    let heir = fixture_user(store).await;
    let post = store
        .read()
        .content()
        .insert(&new_post(&author, "authored"))
        .await
        .unwrap();
    let theirs = store
        .read()
        .content()
        .insert(&new_post(&heir, "theirs"))
        .await
        .unwrap();
    let revs = [
        revision_by(&post, author.id),
        revision_by(&post, author.id),
        revision_by(&theirs, author.id),
        revision_by(&theirs, heir.id),
    ];
    for r in &revs {
        store.read().revisions().insert(r).await.unwrap();
    }
    store
        .read()
        .audit()
        .append(&NewAudit {
            actor_id: Some(author.id),
            action: "user.update".into(),
            target_kind: "user".into(),
            target_id: None,
            details: json!({}),
        })
        .await
        .unwrap();

    // Blocked by the authored post: ForeignKey, and the Tx is poisoned (R18).
    let tx = store.begin().await.unwrap();
    let err = tx.users().delete(author.id).await.unwrap_err();
    assert!(matches!(err, StoreError::ForeignKey { .. }), "{err:?}");
    let next = tx.users().get(author.id).await;
    assert!(matches!(next, Err(StoreError::Aborted)), "{next:?}");
    let commit = tx.commit().await;
    assert!(matches!(commit, Err(StoreError::Aborted)), "{commit:?}");

    // Reassigning only the content still leaves the revisions in the way.
    let tx = store.begin().await.unwrap();
    assert_eq!(
        tx.content()
            .reassign_author(author.id, heir.id)
            .await
            .unwrap(),
        1
    );
    let err = tx.users().delete(author.id).await.unwrap_err();
    assert!(matches!(err, StoreError::ForeignKey { .. }), "{err:?}");
    drop(tx);
    let unchanged = store.read().content().get(post.id).await.unwrap().unwrap();
    assert_eq!(unchanged.author_id, author.id);

    // Both reassigns and the delete in one Tx.
    let tx = store.begin().await.unwrap();
    assert_eq!(
        tx.content()
            .reassign_author(author.id, heir.id)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        tx.revisions()
            .reassign_author(author.id, heir.id)
            .await
            .unwrap(),
        3
    );
    tx.users().delete(author.id).await.unwrap();
    tx.commit().await.unwrap();

    let r = store.read();
    assert!(r.users().get(author.id).await.unwrap().is_none());
    let moved = r.content().get(post.id).await.unwrap().unwrap();
    assert_eq!(moved.author_id, heir.id);
    assert_eq!(
        moved.version,
        post.version + 1,
        "a reassign is a new version"
    );
    let untouched = r.content().get(theirs.id).await.unwrap().unwrap();
    assert_eq!(untouched.version, theirs.version);
    for item in [post.id, theirs.id] {
        let listed = r.revisions().list(item, 10).await.unwrap();
        assert_eq!(listed.len(), 2);
        assert!(listed.iter().all(|rev| rev.author_id == heir.id));
    }
    let audit = r.audit().list(10, None).await.unwrap().items;
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].actor_id, None);

    // Nothing left to move; deleting again is a no-op.
    assert_eq!(
        r.content()
            .reassign_author(author.id, heir.id)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        r.revisions()
            .reassign_author(author.id, heir.id)
            .await
            .unwrap(),
        0
    );
    r.users().delete(author.id).await.unwrap();
    r.users().delete(UserId::new()).await.unwrap();
}

pub async fn count_active_admins_ignores_disabled_and_other_roles(db: &TestDb) {
    let store = db.store();
    let users = store.read().users();
    assert_eq!(users.count_active_admins().await.unwrap(), 0);
    for (login, role, status) in [
        ("admin1", Role::Admin, UserStatus::Active),
        ("admin-off", Role::Admin, UserStatus::Disabled),
        ("editor", Role::Editor, UserStatus::Active),
        ("author", Role::Author, UserStatus::Active),
        ("contrib", Role::Contributor, UserStatus::Active),
    ] {
        insert(store, &user_with(login, role, status))
            .await
            .unwrap();
    }
    assert_eq!(users.count_active_admins().await.unwrap(), 1);
    let second = insert(store, &user_with("admin2", Role::Admin, UserStatus::Active))
        .await
        .unwrap();
    assert_eq!(users.count_active_admins().await.unwrap(), 2);

    let mut demoted = second.clone();
    demoted.role = Role::Editor;
    users.update(&demoted).await.unwrap();
    assert_eq!(users.count_active_admins().await.unwrap(), 1);
    let mut first = users.get_by_login("admin1").await.unwrap().unwrap();
    first.status = UserStatus::Disabled;
    users.update(&first).await.unwrap();
    assert_eq!(users.count_active_admins().await.unwrap(), 0);

    // Inside a Tx the count sees the Tx's own writes.
    let tx = store.begin().await.unwrap();
    first.status = UserStatus::Active;
    tx.users().update(&first).await.unwrap();
    assert_eq!(tx.users().count_active_admins().await.unwrap(), 1);
    tx.rollback().await.unwrap();
    assert_eq!(users.count_active_admins().await.unwrap(), 0);
}

/// F5: `get_for_update` locks the user row for the rest of the `Tx` (Postgres), so a
/// read-modify-write cannot lose a concurrent change.
pub async fn user_get_for_update_locks_the_row(db: Arc<TestDb>) {
    let store = db.shared_store();
    let user = insert(store.as_ref(), &new_user("locked", "locked@example.com"))
        .await
        .unwrap();
    let users = store.read().users();
    assert_eq!(
        users.get_for_update(user.id).await.unwrap().as_ref(),
        Some(&user),
        "outside a Tx it is `get`"
    );
    assert!(users.get_for_update(UserId::new()).await.unwrap().is_none());

    let tx = store.begin().await.unwrap();
    let mut row = tx.users().get_for_update(user.id).await.unwrap().unwrap();
    assert_eq!(row, user);
    let Some(peer) = db.peer().await else {
        // SQLite: one writer; the Tx's own writes still work.
        row.display_name = "In Tx".into();
        tx.users().update(&row).await.unwrap();
        tx.rollback().await.unwrap();
        return;
    };
    let id = user.id;
    let write = tokio::spawn(async move {
        let ptx = peer.begin().await.unwrap();
        let mut theirs = ptx.users().get_for_update(id).await.unwrap().unwrap();
        theirs.status = UserStatus::Disabled;
        ptx.users().update(&theirs).await.unwrap();
        ptx.commit().await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !write.is_finished(),
        "the peer's locking read waits for the row lock"
    );
    row.display_name = "In Tx".into();
    tx.users().update(&row).await.unwrap();
    tx.commit().await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), write)
        .await
        .unwrap()
        .unwrap();
    // The peer read the row after the commit, so neither change is lost.
    let got = store.read().users().get(user.id).await.unwrap().unwrap();
    assert_eq!(got.display_name, "In Tx");
    assert_eq!(got.status, UserStatus::Disabled);
}
