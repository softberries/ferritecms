//! UserService on both backends: validation, the last-admin rule, deletion with
//! reassignment, session/token revocation and the operator (CLI) path (P14–P17).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{collections::BTreeSet, sync::Arc};

use ferrite_core::{
    Actor, CoreError, NewAccount, UserPatch, UserService,
    auth::password::{self, Verified},
    events::user_key,
};
use ferrite_domain::{
    ApiToken, ApiTokenId, Revision, RevisionId, Role, Secret, Session, Timestamp, User, UserId,
    UserStatus,
};
use ferrite_store::Store;
use ferrite_store_tests::{TestDb, backend_tests, backend_tests_spawning, fixtures::new_post};
use serde_json::json;
use tokio::sync::Barrier;

const PASSWORD: &str = "correct horse battery";

fn account(login: &str, role: Role) -> NewAccount {
    NewAccount {
        login: login.into(),
        email: format!("{login}@example.com"),
        display_name: format!("User {login}"),
        role,
        password: None,
        must_reset: false,
    }
}

fn service(db: &TestDb) -> UserService {
    UserService::new(db.shared_store())
}

/// The first admin, created through the operator path (no actor yet).
async fn bootstrap_admin(users: &UserService, login: &str) -> (User, Actor) {
    let user = users
        .operator()
        .create(account(login, Role::Admin))
        .await
        .unwrap();
    let actor = Actor::new(user.id, user.role);
    (user, actor)
}

fn invalid(r: Result<impl std::fmt::Debug, CoreError>) -> String {
    match r {
        Err(CoreError::Invalid(m) | CoreError::InvalidField { message: m, .. }) => m,
        other => panic!("expected Invalid, got {other:?}"),
    }
}

async fn stored(store: &dyn Store, id: UserId) -> User {
    store.read().users().get(id).await.unwrap().unwrap()
}

async fn add_session(store: &dyn Store, user: UserId, id_hash: &str) {
    let now = Timestamp::now();
    store
        .read()
        .sessions()
        .insert(&Session {
            id_hash: id_hash.into(),
            user_id: user,
            csrf_secret: Secret::new(format!("csrf-{id_hash}")),
            created_at: now,
            last_seen_at: now,
            expires_at: Timestamp::from_micros(now.as_micros() + 3_600_000_000).unwrap(),
            ip_hash: None,
            user_agent: None,
        })
        .await
        .unwrap();
}

async fn add_token(store: &dyn Store, user: UserId, hash: &str) {
    store
        .read()
        .api_tokens()
        .insert(&ApiToken {
            id: ApiTokenId::new(),
            user_id: user,
            name: format!("token {hash}"),
            token_hash: hash.into(),
            scopes: vec!["content:read".into()],
            created_at: Timestamp::now(),
            last_used_at: None,
            expires_at: None,
        })
        .await
        .unwrap();
}

async fn session_ids(store: &dyn Store, user: UserId) -> BTreeSet<String> {
    let sessions = store.read().sessions().list_for_user(user).await.unwrap();
    sessions.into_iter().map(|s| s.id_hash).collect()
}

async fn token_count(store: &dyn Store, user: UserId) -> u64 {
    store
        .read()
        .api_tokens()
        .count_for_user(user)
        .await
        .unwrap()
}

async fn audit_actions(store: &dyn Store) -> Vec<String> {
    let page = store.read().audit().list(200, None).await.unwrap();
    page.items.into_iter().map(|a| a.action).collect()
}

async fn seq(store: &dyn Store) -> i64 {
    store.read().events().max_seq().await.unwrap()
}

async fn keys_since(store: &dyn Store, after: i64) -> BTreeSet<String> {
    let events = store.read().events().since(after, 1000).await.unwrap();
    for e in &events {
        assert_eq!(e.kind, "invalidate", "{e:?}");
    }
    events.into_iter().flat_map(|e| e.keys).collect()
}

async fn login_and_email_validation(db: &TestDb) {
    let users = service(db);
    let (_, admin) = bootstrap_admin(&users, "root").await;

    let mut alice = account("  Alice ", Role::Author);
    alice.email = " Alice@Example.com ".into();
    alice.display_name = "  Alice A.  ".into();
    let alice = users.create(&admin, alice).await.unwrap();
    assert_eq!(alice.login, "alice");
    assert_eq!(alice.email, "Alice@Example.com");
    assert_eq!(alice.display_name, "Alice A.");
    assert_eq!(
        db.store()
            .read()
            .users()
            .get_by_login("alice")
            .await
            .unwrap()
            .unwrap()
            .id,
        alice.id
    );

    let long = "a".repeat(61);
    for bad in ["a/b", "-a", long.as_str(), "é", "", "a b", ".a", "_a"] {
        let r = users.create(&admin, account(bad, Role::Author)).await;
        assert!(invalid(r).contains("login"), "{bad:?}");
    }
    let max = "a".repeat(60);
    users
        .create(&admin, account(&max, Role::Author))
        .await
        .unwrap();
    users
        .create(&admin, account("b.c_d-9", Role::Author))
        .await
        .unwrap();

    let long_local = format!("{}@example.com", "x".repeat(243));
    assert_eq!(long_local.len(), 255);
    for bad in [
        "plain",
        "a@b@c",
        "@example.com",
        "a@",
        "a b@example.com",
        "a@exa\tmple.com",
        "a\0@example.com",
        long_local.as_str(),
    ] {
        let mut a = account("mailcheck", Role::Author);
        a.email = bad.into();
        let r = users.create(&admin, a).await;
        assert!(invalid(r).contains("e-mail"), "{bad:?}");
    }

    for bad in ["", "   ", "a\u{7}b", &"n".repeat(101)] {
        let mut a = account("namecheck", Role::Author);
        a.display_name = bad.into();
        let r = users.create(&admin, a).await;
        assert!(invalid(r).contains("display name"), "{bad:?}");
    }

    // Duplicates: the login after normalisation, the e-mail by its ASCII key.
    let mut dup = account("ALICE", Role::Author);
    dup.email = "other@example.com".into();
    assert_eq!(invalid(users.create(&admin, dup).await), "login taken");
    let mut dup = account("alice2", Role::Author);
    dup.email = "alice@EXAMPLE.com".into();
    assert_eq!(invalid(users.create(&admin, dup).await), "e-mail taken");

    // Updating to a taken e-mail; self may change own e-mail and display name.
    let bob = users
        .create(&admin, account("bob", Role::Author))
        .await
        .unwrap();
    let bob_actor = Actor::new(bob.id, bob.role);
    let r = users
        .update(
            &bob_actor,
            bob.id,
            UserPatch {
                email: Some("ALICE@example.com".into()),
                ..UserPatch::default()
            },
        )
        .await;
    assert_eq!(invalid(r), "e-mail taken");
    let bob = users
        .update(
            &bob_actor,
            bob.id,
            UserPatch {
                email: Some("bob@new.example".into()),
                display_name: Some(" Bobby ".into()),
                ..UserPatch::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(bob.email, "bob@new.example");
    assert_eq!(bob.display_name, "Bobby");

    // Bob cannot touch Alice, nor his own role; the admin cannot change their own role.
    assert!(matches!(
        users
            .update(
                &bob_actor,
                alice.id,
                UserPatch {
                    display_name: Some("x".into()),
                    ..UserPatch::default()
                }
            )
            .await,
        Err(CoreError::Forbidden("manage_users"))
    ));
    let r = users
        .update(
            &bob_actor,
            bob.id,
            UserPatch {
                role: Some(Role::Admin),
                ..UserPatch::default()
            },
        )
        .await;
    assert_eq!(invalid(r), "cannot change your own role or status");
    let r = users
        .update(
            &admin,
            admin.user,
            UserPatch {
                status: Some(UserStatus::Disabled),
                ..UserPatch::default()
            },
        )
        .await;
    assert_eq!(invalid(r), "cannot change your own role or status");
    // R10: a self-patch carrying the current role and status is a no-op for them.
    let me = users
        .update(
            &admin,
            admin.user,
            UserPatch {
                display_name: Some("Root Renamed".into()),
                role: Some(Role::Admin),
                status: Some(UserStatus::Active),
                ..UserPatch::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(me.display_name, "Root Renamed");
    assert_eq!(me.role, Role::Admin);
    assert_eq!(me.status, UserStatus::Active);
    let unchanged = users
        .update(
            &bob_actor,
            bob.id,
            UserPatch {
                role: Some(Role::Author),
                ..UserPatch::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(unchanged.role, Role::Author);
    assert!(matches!(
        users.create(&bob_actor, account("eve", Role::Admin)).await,
        Err(CoreError::Forbidden("manage_users"))
    ));
    assert!(matches!(
        users.get(&bob_actor, alice.id).await,
        Err(CoreError::Forbidden("manage_users"))
    ));
    assert_eq!(users.get(&bob_actor, bob.id).await.unwrap().id, bob.id);
    assert!(matches!(
        users.list(&bob_actor, 50, None).await,
        Err(CoreError::Forbidden("manage_users"))
    ));
    assert!(users.list(&admin, 50, None).await.unwrap().items.len() >= 4);

    let actions = audit_actions(db.store()).await;
    assert!(actions.iter().any(|a| a == "user.create"), "{actions:?}");
    assert!(actions.iter().any(|a| a == "user.update"), "{actions:?}");
}

async fn last_admin_is_protected(db: &TestDb) {
    let users = service(db);
    let (root, _) = bootstrap_admin(&users, "root").await;
    // An actor with a stale admin role acting on the only active admin.
    let other = users
        .operator()
        .create(account("helper", Role::Editor))
        .await
        .unwrap();
    let stale_admin = Actor::new(other.id, Role::Admin);
    for patch in [
        UserPatch {
            role: Some(Role::Editor),
            ..UserPatch::default()
        },
        UserPatch {
            status: Some(UserStatus::Disabled),
            ..UserPatch::default()
        },
    ] {
        let r = users.update(&stale_admin, root.id, patch).await;
        assert_eq!(invalid(r), "the site needs at least one active admin");
    }
    let r = users.delete(&stale_admin, root.id, None).await;
    assert_eq!(invalid(r), "the site needs at least one active admin");
    assert_eq!(stored(db.store(), root.id).await.role, Role::Admin);

    // With two active admins, one can be demoted, and then the other is protected.
    let (second, second_actor) = bootstrap_admin(&users, "second").await;
    let root_actor = Actor::new(root.id, Role::Admin);
    let demoted = users
        .update(
            &root_actor,
            second.id,
            UserPatch {
                role: Some(Role::Editor),
                ..UserPatch::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(demoted.role, Role::Editor);
    let r = users
        .update(
            &second_actor,
            root.id,
            UserPatch {
                role: Some(Role::Author),
                ..UserPatch::default()
            },
        )
        .await;
    assert_eq!(invalid(r), "the site needs at least one active admin");
}

async fn delete_requires_reassign_and_moves_content(db: &TestDb) {
    let store = db.store();
    let users = service(db);
    let (_, admin) = bootstrap_admin(&users, "root").await;
    let author = users
        .create(&admin, account("writer", Role::Author))
        .await
        .unwrap();
    let heir = users
        .create(&admin, account("heir", Role::Author))
        .await
        .unwrap();
    let post = store
        .read()
        .content()
        .insert(&new_post(&author, "inherited"))
        .await
        .unwrap();
    store
        .read()
        .revisions()
        .insert(&Revision {
            id: RevisionId::new(),
            item_id: post.id,
            author_id: author.id,
            created_at: Timestamp::now(),
            title: "t".into(),
            excerpt: String::new(),
            body: json!([]),
            fields: json!({}),
            meta: json!({}),
            note: None,
        })
        .await
        .unwrap();

    assert!(matches!(
        users.delete(&admin, admin.user, None).await,
        Err(CoreError::Invalid(_))
    ));
    assert_eq!(
        invalid(users.delete(&admin, author.id, None).await),
        "reassign the user's content first"
    );
    assert!(
        invalid(users.delete(&admin, author.id, Some(UserId::new())).await).contains("reassign")
    );
    assert!(invalid(users.delete(&admin, author.id, Some(author.id)).await).contains("reassign"));
    let disabled = users
        .create(&admin, account("gone", Role::Author))
        .await
        .unwrap();
    users
        .update(
            &admin,
            disabled.id,
            UserPatch {
                status: Some(UserStatus::Disabled),
                ..UserPatch::default()
            },
        )
        .await
        .unwrap();
    assert!(invalid(users.delete(&admin, author.id, Some(disabled.id)).await).contains("active"));
    let bob = Actor::new(heir.id, heir.role);
    assert!(matches!(
        users.delete(&bob, author.id, Some(heir.id)).await,
        Err(CoreError::Forbidden("manage_users"))
    ));
    assert!(store.read().users().get(author.id).await.unwrap().is_some());

    add_session(store, author.id, "writer-session").await;
    add_token(store, author.id, "writer-token").await;
    let before = seq(store).await;
    users
        .delete(&admin, author.id, Some(heir.id))
        .await
        .unwrap();
    assert!(store.read().users().get(author.id).await.unwrap().is_none());
    // Review Focus 3: the deleted user's sessions and tokens are gone.
    assert!(session_ids(store, author.id).await.is_empty());
    assert_eq!(token_count(store, author.id).await, 0);
    assert!(
        store
            .read()
            .sessions()
            .get("writer-session")
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        keys_since(store, before).await,
        [user_key(author.id), user_key(heir.id)]
            .into_iter()
            .collect()
    );
    let moved = store.read().content().get(post.id).await.unwrap().unwrap();
    assert_eq!(moved.author_id, heir.id);
    let revisions = store.read().revisions().list(post.id, 10).await.unwrap();
    assert!(!revisions.is_empty());
    assert!(revisions.iter().all(|r| r.author_id == heir.id));
    let entries = store.read().audit().list(200, None).await.unwrap().items;
    let entry = entries
        .iter()
        .find(|a| a.action == "user.delete")
        .expect("audit row");
    assert_eq!(entry.actor_id, Some(admin.user));
    assert_eq!(
        entry.target_id.as_deref(),
        Some(author.id.to_string().as_str())
    );
    assert_eq!(entry.details["reassign_to"], json!(heir.id.to_string()));

    // A user without content needs no reassignment; deleting a missing user is NotFound.
    let empty = users
        .create(&admin, account("empty", Role::Author))
        .await
        .unwrap();
    users.delete(&admin, empty.id, None).await.unwrap();
    assert!(matches!(
        users.delete(&admin, empty.id, None).await,
        Err(CoreError::NotFound)
    ));
}

async fn password_change_revokes_other_sessions(db: &TestDb) {
    let store = db.store();
    let users = service(db);
    let (_, admin) = bootstrap_admin(&users, "root").await;
    let mut a = account("pat", Role::Author);
    a.password = Some(PASSWORD.into());
    let pat = users.create(&admin, a).await.unwrap();
    let me = Actor::new(pat.id, pat.role);
    add_session(store, pat.id, "session-a").await;
    add_session(store, pat.id, "session-b").await;

    // Wrong or missing current password, and a weak new one.
    for current in [None, Some("wrong password!!")] {
        assert!(matches!(
            users
                .set_password(&me, pat.id, current, "a new password!", None)
                .await,
            Err(CoreError::Forbidden("current password"))
        ));
    }
    assert!(matches!(
        users
            .set_password(&me, pat.id, Some(PASSWORD), "short", None)
            .await,
        Err(CoreError::Invalid(_))
    ));
    assert_eq!(session_ids(store, pat.id).await.len(), 2);

    let new = "a much better passphrase";
    users
        .set_password(&me, pat.id, Some(PASSWORD), new, Some("session-a"))
        .await
        .unwrap();
    assert_eq!(
        session_ids(store, pat.id).await,
        ["session-a".to_owned()].into_iter().collect()
    );
    let row = stored(store, pat.id).await;
    assert!(!row.must_reset);
    assert!(matches!(
        password::verify(new, row.password_hash.as_ref()).await,
        Verified::Match { rehash: false }
    ));
    assert!(matches!(
        password::verify(PASSWORD, row.password_hash.as_ref()).await,
        Verified::Mismatch
    ));

    // An admin setting someone else's password sets must_reset and drops every session.
    add_session(store, pat.id, "session-c").await;
    users
        .set_password(&admin, pat.id, None, "admin chosen secret", None)
        .await
        .unwrap();
    assert!(session_ids(store, pat.id).await.is_empty());
    assert!(stored(store, pat.id).await.must_reset);
    // The user then picks their own: must_reset is cleared.
    users
        .set_password(
            &me,
            pat.id,
            Some("admin chosen secret"),
            "my own secret again",
            None,
        )
        .await
        .unwrap();
    assert!(!stored(store, pat.id).await.must_reset);

    // Another non-admin cannot set it.
    let other = users
        .create(&admin, account("other", Role::Editor))
        .await
        .unwrap();
    assert!(matches!(
        users
            .set_password(
                &Actor::new(other.id, other.role),
                pat.id,
                None,
                "hijacked password",
                None
            )
            .await,
        Err(CoreError::Forbidden("manage_users"))
    ));
    let actions = audit_actions(store).await;
    assert_eq!(actions.iter().filter(|a| *a == "user.password").count(), 3);
}

async fn role_change_and_disable_revoke_sessions_and_tokens(db: &TestDb) {
    let store = db.store();
    let users = service(db);
    let (_, admin) = bootstrap_admin(&users, "root").await;
    let ed = users
        .create(&admin, account("ed", Role::Editor))
        .await
        .unwrap();
    add_session(store, ed.id, "ed-1").await;
    add_token(store, ed.id, "ed-token-1").await;

    // E-mail only: nothing revoked, no event.
    let before = seq(store).await;
    users
        .update(
            &admin,
            ed.id,
            UserPatch {
                email: Some("ed@else.example".into()),
                ..UserPatch::default()
            },
        )
        .await
        .unwrap();
    assert!(keys_since(store, before).await.is_empty());
    assert_eq!(session_ids(store, ed.id).await.len(), 1);

    // Role change: sessions go, tokens stay; no public change, so no event.
    let before = seq(store).await;
    let ed = users
        .update(
            &admin,
            ed.id,
            UserPatch {
                role: Some(Role::Author),
                ..UserPatch::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(ed.role, Role::Author);
    assert!(session_ids(store, ed.id).await.is_empty());
    assert_eq!(token_count(store, ed.id).await, 1);
    assert!(keys_since(store, before).await.is_empty());

    // Display-name change emits user:<id>.
    let before = seq(store).await;
    users
        .update(
            &admin,
            ed.id,
            UserPatch {
                display_name: Some("Edward".into()),
                ..UserPatch::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        keys_since(store, before).await,
        [user_key(ed.id)].into_iter().collect()
    );

    // Disable: sessions and tokens go, user:<id> emitted.
    add_session(store, ed.id, "ed-2").await;
    let before = seq(store).await;
    let ed = users
        .update(
            &admin,
            ed.id,
            UserPatch {
                status: Some(UserStatus::Disabled),
                ..UserPatch::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(ed.status, UserStatus::Disabled);
    assert!(session_ids(store, ed.id).await.is_empty());
    assert_eq!(token_count(store, ed.id).await, 0);
    assert_eq!(
        keys_since(store, before).await,
        [user_key(ed.id)].into_iter().collect()
    );

    // A narrow token of the admin cannot manage users.
    let narrow =
        admin.with_scopes(ferrite_core::Scopes::from_strs(&["content:write".to_owned()]).unwrap());
    assert!(matches!(
        users
            .update(
                &narrow,
                ed.id,
                UserPatch {
                    status: Some(UserStatus::Active),
                    ..UserPatch::default()
                }
            )
            .await,
        Err(CoreError::Forbidden("users:write"))
    ));
    assert!(matches!(
        users.list(&narrow, 10, None).await,
        Err(CoreError::Forbidden("users:read"))
    ));
}

async fn operator_path_bypasses_actor(db: &TestDb) {
    let store = db.store();
    let users = service(db);
    let mut a = account("Op", Role::Admin);
    a.password = Some(PASSWORD.into());
    a.must_reset = true;
    let op = users.operator().create(a).await.unwrap();
    assert_eq!(op.login, "op");
    assert!(op.must_reset);
    assert_eq!(
        invalid(users.operator().create(account("op", Role::Author)).await),
        "login taken"
    );

    store
        .read()
        .users()
        .set_totp(op.id, Some(&Secret::new("sealed")))
        .await
        .unwrap();
    assert!(
        store
            .read()
            .users()
            .advance_totp_step(op.id, 42)
            .await
            .unwrap()
    );
    let row = stored(store, op.id).await;
    assert!(row.totp_secret.is_some());
    assert_eq!(row.totp_last_step, Some(42));
    users.operator().disable_totp(" OP ").await.unwrap();
    let row = stored(store, op.id).await;
    assert!(row.totp_secret.is_none());
    assert_eq!(row.totp_last_step, None);

    add_session(store, op.id, "op-1").await;
    users
        .operator()
        .set_password("op", "operator reset pw", false)
        .await
        .unwrap();
    let row = stored(store, op.id).await;
    assert!(!row.must_reset);
    assert!(session_ids(store, op.id).await.is_empty());
    assert!(matches!(
        password::verify("operator reset pw", row.password_hash.as_ref()).await,
        Verified::Match { .. }
    ));
    assert!(matches!(
        users
            .operator()
            .set_password("nobody", "operator reset pw", true)
            .await,
        Err(CoreError::NotFound)
    ));
    assert!(matches!(
        users.operator().set_password("op", "short", true).await,
        Err(CoreError::Invalid(_))
    ));
    assert!(matches!(
        users.operator().disable_totp("nobody").await,
        Err(CoreError::NotFound)
    ));
    let page = users.operator().list(50, None).await.unwrap();
    assert_eq!(page.items.len(), 1);

    let entries = store.read().audit().list(200, None).await.unwrap().items;
    assert!(entries.iter().all(|e| e.actor_id.is_none()), "{entries:?}");
    let actions: Vec<&str> = entries.iter().map(|e| e.action.as_str()).collect();
    assert!(actions.contains(&"user.create"));
    assert!(actions.contains(&"user.password"));
    assert!(actions.contains(&"user.totp_disable"));
    // No secret ever lands in the details.
    for e in &entries {
        let text = e.details.to_string();
        assert!(
            !text.contains(PASSWORD) && !text.contains("$argon2"),
            "{text}"
        );
    }
}

/// R10: two deletes reassigning to each other, on two nodes (Postgres `peer()`; one
/// store on SQLite), lock in id order and serialize: one wins, the other fails cleanly.
async fn mutual_reassign_deletes_serialize(db: Arc<TestDb>) {
    let one = db.shared_store();
    let two = db.peer().await.unwrap_or_else(|| db.shared_store());
    let (_, admin) = bootstrap_admin(&UserService::new(one.clone()), "root").await;
    for round in 0..5 {
        let users = UserService::new(one.clone());
        let x = users
            .create(&admin, account(&format!("x{round}"), Role::Author))
            .await
            .unwrap();
        let y = users
            .create(&admin, account(&format!("y{round}"), Role::Author))
            .await
            .unwrap();
        let px = one
            .read()
            .content()
            .insert(&new_post(&x, &format!("px{round}")))
            .await
            .unwrap();
        let py = one
            .read()
            .content()
            .insert(&new_post(&y, &format!("py{round}")))
            .await
            .unwrap();

        let barrier = Arc::new(Barrier::new(2));
        let run = |store: Arc<dyn Store>, id: UserId, heir: UserId| {
            let barrier = barrier.clone();
            tokio::spawn(async move {
                let users = UserService::new(store);
                barrier.wait().await;
                users.delete(&admin, id, Some(heir)).await
            })
        };
        let a = run(one.clone(), x.id, y.id);
        let b = run(two.clone(), y.id, x.id);
        let results = [a.await.unwrap(), b.await.unwrap()];
        let ok = results.iter().filter(|r| r.is_ok()).count();
        assert_eq!(ok, 1, "round {round}: {results:?}");
        for r in &results {
            assert!(
                matches!(r, Ok(()) | Err(CoreError::NotFound | CoreError::Invalid(_))),
                "round {round}: {r:?}"
            );
        }
        let survivor = if results[0].is_ok() { y.id } else { x.id };
        let gone = if results[0].is_ok() { x.id } else { y.id };
        assert!(one.read().users().get(gone).await.unwrap().is_none());
        assert!(one.read().users().get(survivor).await.unwrap().is_some());
        for post in [px.id, py.id] {
            let item = one.read().content().get(post).await.unwrap().unwrap();
            assert_eq!(item.author_id, survivor, "round {round}");
        }
    }
}

backend_tests_spawning!(mutual_reassign_deletes_serialize);

backend_tests!(
    login_and_email_validation,
    last_admin_is_protected,
    delete_requires_reassign_and_moves_content,
    password_change_revokes_other_sessions,
    role_change_and_disable_revoke_sessions_and_tokens,
    operator_path_bypasses_actor,
);
