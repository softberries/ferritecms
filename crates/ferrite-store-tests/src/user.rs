//! User contract tests.

use ferrite_domain::{NewUser, Role, Secret, Timestamp, User, UserId, UserStatus};
use ferrite_store::{Store, StoreError};

use crate::{TestDb, fixtures::new_post};

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
    alice.totp_secret = Some(Secret::new("JBSWY3DPEHPK3PXP"));
    alice.last_login_at = Some(Timestamp::now());
    users.update(&alice).await.unwrap();
    let got = users.get(alice.id).await.unwrap().unwrap();
    assert_eq!(got, alice);
    assert_eq!(
        got.totp_secret.as_ref().unwrap().expose(),
        "JBSWY3DPEHPK3PXP"
    );
    assert!(users.get_by_login("alice").await.unwrap().is_none());

    // Clearing optional fields works too.
    alice.totp_secret = None;
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
