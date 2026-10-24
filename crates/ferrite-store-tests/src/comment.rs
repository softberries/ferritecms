//! Contract tests for the comment repository (M2a-2b Task 1, P11, R2/F19).

use std::collections::BTreeSet;

use ferrite_domain::{
    Comment, CommentId, CommentQuery, CommentStatus, ContentId, NewUser, Role, Timestamp, User,
    UserId, UserStatus,
};
use ferrite_store::{AUDIT_CURSOR_TAG, COMMENT_CURSOR_TAG, Store, StoreError, encode_audit_cursor};
use uuid::Uuid;

use crate::{
    TestDb,
    fixtures::{fixture_user, new_post},
};

fn at(micros: i64) -> Timestamp {
    Timestamp::from_micros(micros).unwrap()
}

fn comment(item: ContentId, status: CommentStatus, created_at: Timestamp) -> Comment {
    Comment {
        id: CommentId::new(),
        item_id: item,
        parent_id: None,
        user_id: None,
        author_name: "Ann ☃".into(),
        author_email: "ann@example.com".into(),
        author_url: String::new(),
        body: "Hello <b>there</b>\nsecond line".into(),
        status,
        ip_hash: Some("0123456789abcdef0123456789abcdef".into()),
        created_at,
    }
}

async fn post(store: &dyn Store, author: &User, slug: &str) -> ContentId {
    store
        .read()
        .content()
        .insert(&new_post(author, slug))
        .await
        .unwrap()
        .id
}

fn is_invalid<T: std::fmt::Debug>(r: Result<T, StoreError>) -> bool {
    matches!(r, Err(StoreError::Invalid(_)))
}

pub async fn comment_crud_and_status(db: &TestDb) {
    let store = db.store();
    let repo = store.read().comments();
    let author = fixture_user(store).await;
    let item = post(store, &author, "commented").await;

    let mut first = comment(item, CommentStatus::Pending, Timestamp::now());
    first.author_email = "Ann@Example.COM".into();
    first.author_url = "https://ann.example/".into();
    repo.insert(&first).await.unwrap();
    let got = repo.get(first.id).await.unwrap().unwrap();
    // The e-mail is stored ASCII-lowercased; everything else as given.
    assert_eq!(got.author_email, "ann@example.com");
    assert_eq!(
        got,
        Comment {
            author_email: "ann@example.com".into(),
            ..first.clone()
        }
    );
    assert!(repo.get(CommentId::new()).await.unwrap().is_none());
    assert!(matches!(
        repo.insert(&first).await,
        Err(StoreError::UniqueViolation { constraint }) if constraint == "comments_pkey"
    ));

    // set_status returns the new row.
    let approved = repo
        .set_status(first.id, CommentStatus::Approved)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(approved.status, CommentStatus::Approved);
    assert_eq!(approved.body, first.body);
    assert_eq!(repo.get(first.id).await.unwrap().unwrap(), approved);
    assert!(
        repo.set_status(CommentId::new(), CommentStatus::Spam)
            .await
            .unwrap()
            .is_none()
    );

    // A reply references its parent; an unknown parent, item or user is a foreign key.
    let mut reply = comment(item, CommentStatus::Approved, Timestamp::now());
    reply.parent_id = Some(first.id);
    reply.ip_hash = None;
    repo.insert(&reply).await.unwrap();
    assert_eq!(repo.get(reply.id).await.unwrap().unwrap(), reply);
    let mut orphan = comment(item, CommentStatus::Pending, Timestamp::now());
    orphan.parent_id = Some(CommentId::new());
    assert!(matches!(
        repo.insert(&orphan).await,
        Err(StoreError::ForeignKey { .. })
    ));
    let no_item = comment(ContentId::new(), CommentStatus::Pending, Timestamp::now());
    assert!(matches!(
        repo.insert(&no_item).await,
        Err(StoreError::ForeignKey { .. })
    ));
    let mut no_user = comment(item, CommentStatus::Pending, Timestamp::now());
    no_user.user_id = Some(UserId::new());
    assert!(matches!(
        repo.insert(&no_user).await,
        Err(StoreError::ForeignKey { .. })
    ));

    // Deleting the parent removes its replies; a second delete reports nothing deleted.
    assert!(repo.delete(first.id).await.unwrap());
    assert!(repo.get(reply.id).await.unwrap().is_none());
    assert!(!repo.delete(first.id).await.unwrap());

    // A deleted user leaves the comment with no user.
    let commenter = store
        .read()
        .users()
        .insert(&NewUser {
            id: UserId::new(),
            login: "commenter".into(),
            email: "commenter@example.com".into(),
            display_name: "C".into(),
            password_hash: None,
            role: Role::Contributor,
            status: UserStatus::Active,
            must_reset: false,
        })
        .await
        .unwrap();
    let mut signed = comment(item, CommentStatus::Approved, Timestamp::now());
    signed.user_id = Some(commenter.id);
    repo.insert(&signed).await.unwrap();
    store.read().users().delete(commenter.id).await.unwrap();
    assert_eq!(repo.get(signed.id).await.unwrap().unwrap().user_id, None);

    // Deleting the item removes its comments.
    store.read().content().delete(item).await.unwrap();
    assert!(repo.get(signed.id).await.unwrap().is_none());

    // NUL in any text is refused before any SQL.
    let item = post(store, &author, "nul").await;
    for f in [
        |c: &mut Comment| c.author_name = "a\0".into(),
        |c: &mut Comment| c.author_email = "a\0@x".into(),
        |c: &mut Comment| c.author_url = "https://\0".into(),
        |c: &mut Comment| c.body = "b\0".into(),
        |c: &mut Comment| c.ip_hash = Some("\0".into()),
    ] {
        let mut c = comment(item, CommentStatus::Pending, Timestamp::now());
        f(&mut c);
        assert!(is_invalid(repo.insert(&c).await), "{c:?}");
    }
    assert!(is_invalid(repo.has_approved_from("a\0", "n").await));
    assert!(is_invalid(repo.has_approved_from("a@x", "n\0").await));
}

pub async fn comment_list_filters_and_pages(db: &TestDb) {
    let store = db.store();
    let repo = store.read().comments();
    let author = fixture_user(store).await;
    let items = [
        post(store, &author, "list-a").await,
        post(store, &author, "list-b").await,
    ];
    let statuses = CommentStatus::ALL;
    let base = Timestamp::now().as_micros();
    let mut all = Vec::new();
    for i in 0..25usize {
        // Pairs share a timestamp, so the id breaks ties.
        let created = at(base + (i as i64 / 2) * 1000);
        let c = comment(items[i % 2], statuses[i % statuses.len()], created);
        repo.insert(&c).await.unwrap();
        all.push(c);
    }
    all.sort_by_key(|c| std::cmp::Reverse((c.created_at, c.id.as_uuid())));

    let walk = |query: CommentQuery| async move {
        let mut seen = Vec::new();
        let mut q = query;
        loop {
            let page = repo.list(&q).await.unwrap();
            assert!(page.items.len() <= q.limit as usize);
            seen.extend(page.items.iter().map(|c| c.id));
            match page.next_cursor {
                Some(c) => q.cursor = Some(c),
                None => break,
            }
        }
        seen
    };
    let expect = |f: &dyn Fn(&Comment) -> bool| -> Vec<CommentId> {
        all.iter().filter(|c| f(c)).map(|c| c.id).collect()
    };

    let page = repo
        .list(&CommentQuery {
            limit: 10,
            ..CommentQuery::default()
        })
        .await
        .unwrap();
    assert_eq!(page.items.len(), 10);
    assert_eq!(page.items, all[..10]);
    let ids = walk(CommentQuery {
        limit: 10,
        ..CommentQuery::default()
    })
    .await;
    assert_eq!(ids.len(), 25);
    assert_eq!(ids.iter().collect::<BTreeSet<_>>().len(), 25, "no repeats");
    assert_eq!(ids, expect(&|_| true));

    for status in statuses {
        let got = walk(CommentQuery {
            status: Some(*status),
            limit: 3,
            ..CommentQuery::default()
        })
        .await;
        assert_eq!(got, expect(&|c| c.status == *status), "{status}");
        assert!(!got.is_empty());
    }
    for item in items {
        let got = walk(CommentQuery {
            item: Some(item),
            limit: 10,
            ..CommentQuery::default()
        })
        .await;
        assert_eq!(got, expect(&|c| c.item_id == item));
    }
    let both = walk(CommentQuery {
        status: Some(CommentStatus::Pending),
        item: Some(items[0]),
        limit: 2,
        cursor: None,
    })
    .await;
    assert_eq!(
        both,
        expect(&|c| c.status == CommentStatus::Pending && c.item_id == items[0])
    );
    assert!(
        walk(CommentQuery {
            item: Some(ContentId::new()),
            limit: 10,
            ..CommentQuery::default()
        })
        .await
        .is_empty()
    );

    // Bad cursors and limits.
    for cursor in [
        "garbage!".to_string(),
        encode_audit_cursor(Timestamp::now(), Uuid::now_v7()),
        ferrite_domain::Cursor::encode(AUDIT_CURSOR_TAG, &serde_json::json!(1), Uuid::now_v7()),
        // The comment tag with a key that is not a timestamp.
        ferrite_domain::Cursor::encode(COMMENT_CURSOR_TAG, &serde_json::json!("x"), Uuid::now_v7()),
        ferrite_domain::Cursor::encode(COMMENT_CURSOR_TAG, &serde_json::json!(1.5), Uuid::now_v7()),
        ferrite_domain::Cursor::encode(
            COMMENT_CURSOR_TAG,
            &serde_json::json!(i64::MIN),
            Uuid::now_v7(),
        ),
        ferrite_domain::Cursor::encode(
            COMMENT_CURSOR_TAG,
            &serde_json::Value::Null,
            Uuid::now_v7(),
        ),
    ] {
        assert!(
            is_invalid(
                repo.list(&CommentQuery {
                    limit: 10,
                    cursor: Some(cursor.clone()),
                    ..CommentQuery::default()
                })
                .await
            ),
            "{cursor}"
        );
    }
    for limit in [0, 201] {
        assert!(is_invalid(
            repo.list(&CommentQuery {
                limit,
                ..CommentQuery::default()
            })
            .await
        ));
    }
}

pub async fn approved_for_item_is_oldest_first_and_capped(db: &TestDb) {
    let store = db.store();
    let repo = store.read().comments();
    let author = fixture_user(store).await;
    let item = post(store, &author, "thread").await;
    let other = post(store, &author, "other-thread").await;
    let base = Timestamp::now().as_micros();
    let mut approved = Vec::new();
    for i in 0..6i64 {
        // Inserted newest first, two per timestamp.
        let status = if i % 3 == 2 {
            CommentStatus::Pending
        } else {
            CommentStatus::Approved
        };
        let c = comment(item, status, at(base - (i / 2) * 1000));
        repo.insert(&c).await.unwrap();
        if status == CommentStatus::Approved {
            approved.push(c);
        }
    }
    repo.insert(&comment(other, CommentStatus::Approved, at(base)))
        .await
        .unwrap();
    approved.sort_by_key(|c| (c.created_at, c.id.as_uuid()));

    assert_eq!(repo.approved_for_item(item, 500).await.unwrap(), approved);
    assert_eq!(
        repo.approved_for_item(item, 2).await.unwrap(),
        approved[..2]
    );
    assert!(
        repo.approved_for_item(ContentId::new(), 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(is_invalid(repo.approved_for_item(item, 0).await));
    assert!(is_invalid(repo.approved_for_item(item, 501).await));
}

pub async fn has_approved_from_matches_exact_lowercase(db: &TestDb) {
    let store = db.store();
    let repo = store.read().comments();
    let author = fixture_user(store).await;
    let item = post(store, &author, "known").await;

    let mut ann = comment(item, CommentStatus::Approved, Timestamp::now());
    ann.author_name = "Ann".into();
    ann.author_email = "Ann@Example.com".into();
    repo.insert(&ann).await.unwrap();
    let mut bob = comment(item, CommentStatus::Pending, Timestamp::now());
    bob.author_name = "Bob".into();
    bob.author_email = "bob@example.com".into();
    repo.insert(&bob).await.unwrap();
    let mut spam = comment(item, CommentStatus::Spam, Timestamp::now());
    spam.author_name = "Sam".into();
    spam.author_email = "sam@example.com".into();
    repo.insert(&spam).await.unwrap();
    let mut accent = comment(item, CommentStatus::Approved, Timestamp::now());
    accent.author_name = "É".into();
    accent.author_email = "É@example.com".into();
    repo.insert(&accent).await.unwrap();

    // The e-mail folds ASCII case only; the name is exact.
    assert!(
        repo.has_approved_from("ann@example.com", "Ann")
            .await
            .unwrap()
    );
    assert!(
        repo.has_approved_from("ANN@EXAMPLE.COM", "Ann")
            .await
            .unwrap()
    );
    assert!(
        !repo
            .has_approved_from("ann@example.com", "ann")
            .await
            .unwrap()
    );
    assert!(
        !repo
            .has_approved_from("ann@example.com", "Ann ")
            .await
            .unwrap()
    );
    assert!(
        !repo
            .has_approved_from("ann@example.co", "Ann")
            .await
            .unwrap()
    );
    assert!(
        !repo
            .has_approved_from("bob@example.com", "Bob")
            .await
            .unwrap()
    );
    assert!(
        !repo
            .has_approved_from("sam@example.com", "Sam")
            .await
            .unwrap()
    );
    assert!(repo.has_approved_from("É@example.com", "É").await.unwrap());
    assert!(!repo.has_approved_from("é@example.com", "É").await.unwrap());

    // A signed-in commenter's approved comment does not vouch for an anonymous one with
    // the same name and e-mail; it is matched by user id instead.
    let user = fixture_user(store).await;
    assert!(!repo.has_approved_from_user(user.id).await.unwrap());
    let mut signed = comment(item, CommentStatus::Approved, Timestamp::now());
    signed.user_id = Some(user.id);
    signed.author_name = "Carl".into();
    signed.author_email = "carl@example.com".into();
    repo.insert(&signed).await.unwrap();
    assert!(
        !repo
            .has_approved_from("carl@example.com", "Carl")
            .await
            .unwrap()
    );
    assert!(repo.has_approved_from_user(user.id).await.unwrap());
    assert!(!repo.has_approved_from_user(author.id).await.unwrap());
    repo.set_status(signed.id, CommentStatus::Trash)
        .await
        .unwrap();
    assert!(!repo.has_approved_from_user(user.id).await.unwrap());
}
