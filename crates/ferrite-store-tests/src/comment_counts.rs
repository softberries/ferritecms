//! Contract tests for `CommentRepo::counts` (M2b-2 Task 3): one count per status,
//! zero-filled, over every comment or one item's.

use std::collections::BTreeMap;

use ferrite_domain::{Comment, CommentId, CommentStatus, ContentId, Timestamp};

use crate::{
    TestDb,
    fixtures::{fixture_user, new_post},
};

fn comment(item: ContentId, status: CommentStatus) -> Comment {
    Comment {
        id: CommentId::new(),
        item_id: item,
        parent_id: None,
        user_id: None,
        author_name: "Ann".into(),
        author_email: "ann@example.com".into(),
        author_url: String::new(),
        body: "Hello".into(),
        status,
        ip_hash: None,
        created_at: Timestamp::now(),
    }
}

fn counts(pending: u64, approved: u64, spam: u64, trash: u64) -> BTreeMap<CommentStatus, u64> {
    BTreeMap::from([
        (CommentStatus::Pending, pending),
        (CommentStatus::Approved, approved),
        (CommentStatus::Spam, spam),
        (CommentStatus::Trash, trash),
    ])
}

pub async fn comment_counts_per_status_and_item(db: &TestDb) {
    let store = db.store();
    let repo = store.read().comments();
    // No comments: every status present, all zero.
    assert_eq!(repo.counts(None).await.unwrap(), counts(0, 0, 0, 0));

    let author = fixture_user(store).await;
    let content = store.read().content();
    let a = content.insert(&new_post(&author, "a")).await.unwrap().id;
    let b = content.insert(&new_post(&author, "b")).await.unwrap().id;
    let quiet = content
        .insert(&new_post(&author, "quiet"))
        .await
        .unwrap()
        .id;
    for (item, status) in [
        (a, CommentStatus::Pending),
        (a, CommentStatus::Pending),
        (a, CommentStatus::Approved),
        (a, CommentStatus::Spam),
        (b, CommentStatus::Pending),
        (b, CommentStatus::Trash),
        (b, CommentStatus::Trash),
    ] {
        repo.insert(&comment(item, status)).await.unwrap();
    }

    assert_eq!(repo.counts(None).await.unwrap(), counts(3, 1, 1, 2));
    assert_eq!(repo.counts(Some(a)).await.unwrap(), counts(2, 1, 1, 0));
    assert_eq!(repo.counts(Some(b)).await.unwrap(), counts(1, 0, 0, 2));
    assert_eq!(repo.counts(Some(quiet)).await.unwrap(), counts(0, 0, 0, 0));
    assert_eq!(
        repo.counts(Some(ContentId::new())).await.unwrap(),
        counts(0, 0, 0, 0)
    );

    // A status change moves the count; inside a Tx too.
    let tx = store.begin().await.unwrap();
    let held = comment(a, CommentStatus::Pending);
    tx.comments().insert(&held).await.unwrap();
    tx.comments()
        .set_status(held.id, CommentStatus::Approved)
        .await
        .unwrap();
    assert_eq!(
        tx.comments().counts(Some(a)).await.unwrap(),
        counts(2, 2, 1, 0)
    );
    tx.commit().await.unwrap();
    assert_eq!(repo.counts(None).await.unwrap(), counts(3, 2, 1, 2));
}
