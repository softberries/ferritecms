//! M2b-3 Task 1: `UserRepo::list(&UserQuery)` filters (P46) and the sessions' public ids
//! (P45).

use ferrite_domain::{
    NewUser, Role, Secret, Session, Timestamp, User, UserId, UserQuery, UserStatus,
};
use ferrite_store::{Store, StoreError, session_public_id};

use crate::TestDb;

fn new_user(login: &str, display_name: &str, email: &str, role: Role) -> NewUser {
    NewUser {
        id: UserId::new(),
        login: login.into(),
        email: email.into(),
        display_name: display_name.into(),
        password_hash: None,
        role,
        status: UserStatus::Active,
        must_reset: false,
    }
}

/// Every login matching `query`, following the cursor page by page.
async fn logins(store: &dyn Store, query: UserQuery) -> Vec<String> {
    let mut query = query;
    let mut seen = Vec::new();
    loop {
        let page = store.read().users().list(&query).await.unwrap();
        seen.extend(page.items.iter().map(|u| u.login.clone()));
        match page.next_cursor {
            Some(c) => query.cursor = Some(c),
            None => return seen,
        }
    }
}

fn q(text: &str) -> UserQuery {
    UserQuery {
        q: Some(text.into()),
        limit: 50,
        ..UserQuery::default()
    }
}

pub async fn user_list_filters(db: &TestDb) {
    let store = db.store();
    let users = store.read().users();
    for (login, name, email, role) in [
        ("ada", "Ada Lovelace", "ada@example.com", Role::Admin),
        ("bob", "Bob 50%_x", "bob@mail.test", Role::Editor),
        ("cy", "Cy 50xxx", "cy@example.com", Role::Editor),
        ("dee", "Dee", "DEE@Example.ORG", Role::Author),
        ("eve", "Eve back\\slash", "eve@example.com", Role::Editor),
        ("fay", "Fay", "fay@lovelace.test", Role::Editor),
    ] {
        users
            .insert(&new_user(login, name, email, role))
            .await
            .unwrap();
    }
    let mut cy = users.get_by_login("cy").await.unwrap().unwrap();
    cy.status = UserStatus::Disabled;
    users.update(&cy).await.unwrap();

    // `q` over login, display name and e-mail, case-insensitively.
    assert_eq!(logins(store, q("ADA")).await, ["ada"]);
    assert_eq!(logins(store, q("lovelace")).await, ["ada", "fay"]);
    assert_eq!(logins(store, q("example.org")).await, ["dee"]);
    assert_eq!(logins(store, q("mail.test")).await, ["bob"]);
    // Wildcards match only themselves.
    assert_eq!(logins(store, q("50%_x")).await, ["bob"]);
    assert_eq!(logins(store, q("%")).await, ["bob"]);
    assert_eq!(logins(store, q("_")).await, ["bob"]);
    assert_eq!(logins(store, q("\\")).await, ["eve"]);
    assert!(logins(store, q("nobody")).await.is_empty());

    // Role and status, alone and combined with `q`, through the cursor.
    let editors = UserQuery {
        role: Some(Role::Editor),
        limit: 1,
        ..UserQuery::default()
    };
    assert_eq!(
        logins(store, editors.clone()).await,
        ["bob", "cy", "eve", "fay"]
    );
    let active_editors = UserQuery {
        status: Some(UserStatus::Active),
        ..editors.clone()
    };
    assert_eq!(
        logins(store, active_editors.clone()).await,
        ["bob", "eve", "fay"]
    );
    let disabled = UserQuery {
        status: Some(UserStatus::Disabled),
        limit: 50,
        ..UserQuery::default()
    };
    assert_eq!(logins(store, disabled).await, ["cy"]);
    let searched = UserQuery {
        q: Some("e".into()),
        ..active_editors
    };
    // "e" is in bob's e-mail, eve's login and fay's e-mail ("lovelace.test").
    assert_eq!(logins(store, searched).await, ["bob", "eve", "fay"]);
    let first = users
        .list(&UserQuery {
            role: Some(Role::Editor),
            status: Some(UserStatus::Active),
            limit: 2,
            ..UserQuery::default()
        })
        .await
        .unwrap();
    assert_eq!(
        first
            .items
            .iter()
            .map(|u| u.login.as_str())
            .collect::<Vec<_>>(),
        ["bob", "eve"]
    );
    assert!(first.next_cursor.is_some());
    // No filter: every user.
    let all = UserQuery {
        limit: 50,
        ..UserQuery::default()
    };
    assert_eq!(
        logins(store, all).await,
        ["ada", "bob", "cy", "dee", "eve", "fay"]
    );

    // A NUL in `q` and a bad limit or cursor are refused.
    assert!(matches!(
        users.list(&q("a\0b")).await,
        Err(StoreError::Invalid(_))
    ));
    assert!(matches!(
        users
            .list(&UserQuery {
                limit: 0,
                ..UserQuery::default()
            })
            .await,
        Err(StoreError::Invalid(_))
    ));
    assert!(matches!(
        users
            .list(&UserQuery {
                limit: 2,
                cursor: Some("not a cursor".into()),
                ..UserQuery::default()
            })
            .await,
        Err(StoreError::Invalid(_))
    ));
}

fn session(user: &User, id_hash: &str, created_at: Timestamp) -> Session {
    Session {
        id_hash: id_hash.into(),
        user_id: user.id,
        csrf_secret: Secret::new(format!("csrf-{id_hash}")),
        created_at,
        last_seen_at: created_at,
        expires_at: Timestamp::from_micros(created_at.as_micros() + 3_600_000_000).unwrap(),
        ip_hash: None,
        user_agent: None,
    }
}

pub async fn session_public_id_roundtrip(db: &TestDb) {
    let store = db.store();
    let users = store.read().users();
    let ann = users
        .insert(&new_user("ann", "Ann", "ann@example.com", Role::Author))
        .await
        .unwrap();
    let ben = users
        .insert(&new_user("ben", "Ben", "ben@example.com", Role::Author))
        .await
        .unwrap();
    let sessions = store.read().sessions();
    let now = Timestamp::now();
    let a = session(&ann, &"a".repeat(64), now);
    let b = session(&ann, &"b".repeat(64), now);
    let theirs = session(&ben, &"c".repeat(64), now);
    for s in [&a, &b, &theirs] {
        sessions.insert(s).await.unwrap();
    }

    // 16 lowercase hex characters, stable, distinct, and not a prefix of the stored key.
    let id = session_public_id(&a.id_hash);
    assert_eq!(id.len(), 16);
    assert!(
        id.bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    );
    assert_eq!(id, session_public_id(&a.id_hash));
    assert_ne!(id, session_public_id(&b.id_hash));
    assert!(!a.id_hash.starts_with(&id));

    let got = sessions.get_by_public_id(ann.id, &id).await.unwrap();
    assert_eq!(got, Some(a.clone()));
    // Only the owner's sessions are found.
    let theirs_id = session_public_id(&theirs.id_hash);
    assert_eq!(
        sessions.get_by_public_id(ann.id, &theirs_id).await.unwrap(),
        None
    );
    assert!(
        !sessions
            .delete_by_public_id(ann.id, &theirs_id)
            .await
            .unwrap()
    );
    assert!(sessions.get(&theirs.id_hash).await.unwrap().is_some());
    assert_eq!(
        sessions
            .get_by_public_id(ann.id, "0000000000000000")
            .await
            .unwrap(),
        None
    );

    assert!(sessions.delete_by_public_id(ann.id, &id).await.unwrap());
    assert!(sessions.get(&a.id_hash).await.unwrap().is_none());
    assert!(!sessions.delete_by_public_id(ann.id, &id).await.unwrap());
    // The other session is untouched.
    assert_eq!(sessions.get(&b.id_hash).await.unwrap(), Some(b));
}
