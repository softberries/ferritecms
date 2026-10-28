//! Contract tests for the public comment pages (M2b-2 Task 4, P34):
//! `CommentRepo::approved_page`, `approved_totals` and `root_position`.
//!
//! A root is an approved comment whose parent is null or not approved; roots are ordered
//! `(created_at, id)`; a page holds roots `offset..offset + roots`, each followed by its
//! approved descendants depth first, oldest first.

use std::collections::{HashMap, HashSet};

use ferrite_domain::{Comment, CommentId, CommentStatus, ContentId, Timestamp, User};
use ferrite_store::{ApprovedTotals, MAX_PAGE_ROOTS, MAX_PAGE_ROWS, Store, StoreError};

use crate::{
    TestDb,
    fixtures::{fixture_user, new_post},
};

const BASE: i64 = 1_700_000_000_000_000;

fn at(micros: i64) -> Timestamp {
    Timestamp::from_micros(BASE + micros).unwrap()
}

fn comment(
    item: ContentId,
    parent: Option<CommentId>,
    status: CommentStatus,
    micros: i64,
) -> Comment {
    Comment {
        id: CommentId::new(),
        item_id: item,
        parent_id: parent,
        user_id: None,
        author_name: "Ann".into(),
        author_email: "ann@example.com".into(),
        author_url: String::new(),
        body: format!("at {micros}"),
        status,
        ip_hash: None,
        created_at: at(micros),
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

/// Inserts `rows` in one `Tx`, in the order given.
async fn insert_all(store: &dyn Store, rows: &[Comment]) {
    let tx = store.begin().await.unwrap();
    for r in rows {
        tx.comments().insert(r).await.unwrap();
    }
    tx.commit().await.unwrap();
}

/// The approved comments of `rows` in thread order, one `Vec` per root (P34).
fn threads(rows: &[Comment]) -> Vec<Vec<Comment>> {
    let approved: Vec<&Comment> = rows
        .iter()
        .filter(|c| c.status == CommentStatus::Approved)
        .collect();
    let ids: HashSet<CommentId> = approved.iter().map(|c| c.id).collect();
    let key = |c: &&Comment| (c.created_at, c.id.as_uuid());
    let mut roots: Vec<&Comment> = approved
        .iter()
        .copied()
        .filter(|c| c.parent_id.is_none_or(|p| !ids.contains(&p)))
        .collect();
    roots.sort_by_key(key);
    let mut children: HashMap<CommentId, Vec<&Comment>> = HashMap::new();
    for c in &approved {
        if let Some(p) = c.parent_id.filter(|p| ids.contains(p)) {
            children.entry(p).or_default().push(c);
        }
    }
    for list in children.values_mut() {
        list.sort_by_key(key);
    }
    roots
        .into_iter()
        .map(|root| {
            let mut out = Vec::new();
            let mut stack = vec![root];
            while let Some(c) = stack.pop() {
                out.push(c.clone());
                if let Some(kids) = children.get(&c.id) {
                    stack.extend(kids.iter().rev());
                }
            }
            out
        })
        .collect()
}

fn is_invalid<T: std::fmt::Debug>(r: Result<T, StoreError>) -> bool {
    matches!(r, Err(StoreError::Invalid(_)))
}

/// 120 roots (pairs share a timestamp, inserted newest first), every seventh with a
/// three-deep chain and an older sibling reply; held, spam and trashed replies; an
/// approved reply under a spam comment (a root, with its own approved reply); another
/// item's comments. Pages of 50 roots: 3 pages, each in thread order, together every
/// approved comment exactly once.
pub async fn thread_pages_cover_every_comment(db: &TestDb) {
    let store = db.store();
    let author = fixture_user(store).await;
    let item = post(store, &author, "long-thread").await;
    let other = post(store, &author, "other-thread").await;

    let mut rows = Vec::new();
    for i in (0..120i64).rev() {
        let t = (i / 2) * 1000;
        let root = comment(item, None, CommentStatus::Approved, t);
        if i % 7 == 0 {
            let a = comment(item, Some(root.id), CommentStatus::Approved, t + 10);
            let b = comment(item, Some(a.id), CommentStatus::Approved, t + 20);
            let c = comment(item, Some(b.id), CommentStatus::Approved, t + 30);
            let older = comment(item, Some(root.id), CommentStatus::Approved, t + 5);
            rows.extend([root.clone(), a, b, c, older]);
        } else {
            rows.push(root.clone());
        }
        match i {
            1 => rows.push(comment(item, Some(root.id), CommentStatus::Pending, t + 1)),
            2 => rows.push(comment(item, Some(root.id), CommentStatus::Trash, t + 1)),
            3 => rows.push(comment(item, Some(root.id), CommentStatus::Spam, t + 1)),
            _ => {}
        }
    }
    let spam = comment(item, None, CommentStatus::Spam, 30_500);
    let orphan = comment(item, Some(spam.id), CommentStatus::Approved, 30_600);
    let orphan_reply = comment(item, Some(orphan.id), CommentStatus::Approved, 30_700);
    rows.extend([spam, orphan.clone(), orphan_reply]);
    rows.push(comment(item, None, CommentStatus::Pending, 70_000));
    let others: Vec<Comment> = (0..3)
        .map(|n| comment(other, None, CommentStatus::Approved, n))
        .collect();
    insert_all(store, &rows).await;
    insert_all(store, &others).await;

    let expected = threads(&rows);
    assert_eq!(expected.len(), 121, "120 roots and the reply under spam");
    assert!(expected.iter().flatten().any(|c| c.id == orphan.id));

    let repo = store.read().comments();
    let mut seen = Vec::new();
    for (page, want_roots) in [(0u32, 50usize), (1, 50), (2, 21)] {
        let got = repo
            .approved_page(item, page * 50, 50, MAX_PAGE_ROWS)
            .await
            .unwrap();
        let want: Vec<Comment> = expected
            .iter()
            .skip(page as usize * 50)
            .take(want_roots)
            .flatten()
            .cloned()
            .collect();
        assert_eq!(got, want, "page {page}");
        seen.extend(got.into_iter().map(|c| c.id));
    }
    assert!(
        repo.approved_page(item, 150, 50, MAX_PAGE_ROWS)
            .await
            .unwrap()
            .is_empty()
    );

    let all: Vec<CommentId> = expected.iter().flatten().map(|c| c.id).collect();
    assert_eq!(seen.len(), all.len(), "no comment twice");
    assert_eq!(
        seen.iter().copied().collect::<HashSet<_>>(),
        all.into_iter().collect::<HashSet<_>>()
    );
    assert_eq!(
        repo.approved_page(other, 0, 50, MAX_PAGE_ROWS)
            .await
            .unwrap(),
        threads(&others).concat()
    );
    assert!(
        repo.approved_page(ContentId::new(), 0, 50, MAX_PAGE_ROWS)
            .await
            .unwrap()
            .is_empty()
    );
    // Inside a Tx too.
    let tx = store.begin().await.unwrap();
    assert_eq!(
        tx.comments()
            .approved_page(item, 100, 50, MAX_PAGE_ROWS)
            .await
            .unwrap(),
        expected[100..].concat()
    );
    tx.rollback().await.unwrap();

    assert!(is_invalid(
        repo.approved_page(item, 0, 0, MAX_PAGE_ROWS).await
    ));
    assert!(is_invalid(
        repo.approved_page(item, 0, MAX_PAGE_ROOTS + 1, MAX_PAGE_ROWS)
            .await
    ));
    assert!(is_invalid(repo.approved_page(item, 0, 50, 0).await));
    assert!(is_invalid(
        repo.approved_page(item, 0, 50, MAX_PAGE_ROWS + 1).await
    ));
    assert!(is_invalid(repo.approved_page(item, 0, 50, 49).await));
}

/// A page always holds every one of its roots (amended P34): past the row budget only
/// replies are cut, in thread order (depth first, oldest first), so the kept replies
/// are a prefix of the page's thread order; the next page is unaffected.
pub async fn approved_page_keeps_every_root(db: &TestDb) {
    let store = db.store();
    let author = fixture_user(store).await;
    let item = post(store, &author, "busy-threads").await;

    // 6 roots, each with 9 replies (every third one opening a deeper level): 60 rows.
    let mut rows = Vec::new();
    for r in 0..6i64 {
        let t = r * 1_000_000;
        let root = comment(item, None, CommentStatus::Approved, t);
        let mut parent = root.id;
        rows.push(root);
        for n in 1..10 {
            let c = comment(item, Some(parent), CommentStatus::Approved, t + n);
            if n % 3 == 0 {
                parent = c.id;
            }
            rows.push(c);
        }
    }
    insert_all(store, &rows).await;
    let expected = threads(&rows);
    let repo = store.read().comments();

    // Pages of 3 roots (30 rows each) with a budget of 12 rows.
    for page in 0..2u32 {
        let mine = &expected[page as usize * 3..page as usize * 3 + 3];
        let got = repo.approved_page(item, page * 3, 3, 12).await.unwrap();
        assert_eq!(got.len(), 12, "page {page}: the budget is used up");
        let roots: Vec<CommentId> = mine.iter().map(|t| t[0].id).collect();
        for root in &roots {
            assert!(got.iter().any(|c| c.id == *root), "root {root} kept");
        }
        // Thread order of the whole page, roots always kept, replies a prefix.
        let order: Vec<&Comment> = mine.iter().flatten().collect();
        let replies: Vec<&Comment> = order
            .iter()
            .copied()
            .filter(|c| !roots.contains(&c.id))
            .collect();
        let want: Vec<CommentId> = order
            .iter()
            .filter(|c| roots.contains(&c.id) || replies[..9].iter().any(|r| r.id == c.id))
            .map(|c| c.id)
            .collect();
        let got_ids: Vec<CommentId> = got.iter().map(|c| c.id).collect();
        assert_eq!(got_ids, want, "page {page}");
        // Every reply shown has its parent shown.
        for c in &got {
            if let Some(p) = c.parent_id {
                assert!(got_ids.contains(&p), "{} without its parent", c.id);
            }
        }
    }
    // A budget equal to the roots shows the roots alone; a large one shows everything.
    let bare = repo.approved_page(item, 0, 3, 3).await.unwrap();
    assert_eq!(
        bare.iter().map(|c| c.id).collect::<Vec<_>>(),
        expected[..3].iter().map(|t| t[0].id).collect::<Vec<_>>()
    );
    assert_eq!(
        repo.approved_page(item, 3, 3, MAX_PAGE_ROWS).await.unwrap(),
        expected[3..].concat()
    );
}

/// A short last page (fewer roots than asked for) under a tight row budget: the budget
/// left after its own roots goes to its replies in thread order, so the page fills up to
/// the budget (the smallest budget allowed is the number of roots asked for).
pub async fn short_comment_page_under_tight_budget(db: &TestDb) {
    let store = db.store();
    let author = fixture_user(store).await;
    let item = post(store, &author, "short-page").await;
    // 7 roots, each with a chain of 3 replies: 28 rows.
    let mut rows = Vec::new();
    for r in 0..7i64 {
        let t = r * 1_000_000;
        let root = comment(item, None, CommentStatus::Approved, t);
        let mut parent = root.id;
        rows.push(root);
        for n in 1..4 {
            let c = comment(item, Some(parent), CommentStatus::Approved, t + n);
            parent = c.id;
            rows.push(c);
        }
    }
    insert_all(store, &rows).await;
    let expected = threads(&rows);
    let repo = store.read().comments();

    // Page 2 of 5 roots holds roots 5 and 6 only; a budget of 6 rows leaves 4 replies:
    // root 5's three, then the first of root 6's.
    let got: Vec<CommentId> = repo
        .approved_page(item, 5, 5, 6)
        .await
        .unwrap()
        .iter()
        .map(|c| c.id)
        .collect();
    let mut want: Vec<CommentId> = expected[5].iter().map(|c| c.id).collect();
    want.extend([expected[6][0].id, expected[6][1].id]);
    assert_eq!(got, want);
    // The smallest budget (the roots asked for, 5) leaves 3 replies: root 5's.
    let got: Vec<CommentId> = repo
        .approved_page(item, 5, 5, 5)
        .await
        .unwrap()
        .iter()
        .map(|c| c.id)
        .collect();
    let mut want: Vec<CommentId> = expected[5].iter().map(|c| c.id).collect();
    want.push(expected[6][0].id);
    assert_eq!(got, want);
    // A budget that fits the whole short page shows all of it.
    assert_eq!(
        repo.approved_page(item, 5, 5, 8).await.unwrap(),
        expected[5..].concat()
    );
}

/// The 0-based position of the root whose thread holds a comment; `None` for a comment
/// that is not approved, unknown, or of another item.
pub async fn root_position_of_reply_is_its_root(db: &TestDb) {
    let store = db.store();
    let author = fixture_user(store).await;
    let item = post(store, &author, "positions").await;
    let other = post(store, &author, "elsewhere").await;

    let a = comment(item, None, CommentStatus::Approved, 0);
    let b = comment(item, None, CommentStatus::Approved, 100);
    let b1 = comment(item, Some(b.id), CommentStatus::Approved, 110);
    let b2 = comment(item, Some(b1.id), CommentStatus::Approved, 120);
    let spam = comment(item, None, CommentStatus::Spam, 150);
    let x = comment(item, Some(spam.id), CommentStatus::Approved, 160);
    let held = comment(item, Some(a.id), CommentStatus::Pending, 170);
    let under_held = comment(item, Some(held.id), CommentStatus::Approved, 180);
    let c = comment(item, None, CommentStatus::Approved, 200);
    let elsewhere = comment(other, None, CommentStatus::Approved, 0);
    // Inserted oldest first.
    let rows = [
        a.clone(),
        b.clone(),
        b1.clone(),
        b2.clone(),
        spam.clone(),
        x.clone(),
        held.clone(),
        under_held.clone(),
        c.clone(),
    ];
    insert_all(store, &rows).await;
    insert_all(store, std::slice::from_ref(&elsewhere)).await;

    let repo = store.read().comments();
    let pos = |id: CommentId| repo.root_position(item, id);
    assert_eq!(pos(a.id).await.unwrap(), Some(0));
    assert_eq!(pos(b.id).await.unwrap(), Some(1));
    assert_eq!(pos(b1.id).await.unwrap(), Some(1));
    assert_eq!(pos(b2.id).await.unwrap(), Some(1));
    assert_eq!(pos(x.id).await.unwrap(), Some(2), "a reply under spam");
    assert_eq!(pos(under_held.id).await.unwrap(), Some(3));
    assert_eq!(pos(c.id).await.unwrap(), Some(4));
    assert_eq!(pos(spam.id).await.unwrap(), None);
    assert_eq!(pos(held.id).await.unwrap(), None);
    assert_eq!(pos(CommentId::new()).await.unwrap(), None);
    assert_eq!(pos(elsewhere.id).await.unwrap(), None, "another item's");
    assert_eq!(
        repo.root_position(other, elsewhere.id).await.unwrap(),
        Some(0)
    );

    // Each position is where `approved_page` puts the comment.
    for (id, p) in [(b2.id, 1u32), (x.id, 2), (under_held.id, 3)] {
        let page = repo.approved_page(item, p, 1, MAX_PAGE_ROWS).await.unwrap();
        assert!(page.iter().any(|c| c.id == id), "{id} at {p}");
    }
}

/// Approved comments and roots of one item; zero for none and for an unknown item.
pub async fn approved_totals_counts_all(db: &TestDb) {
    let store = db.store();
    let author = fixture_user(store).await;
    let item = post(store, &author, "totals").await;
    let quiet = post(store, &author, "quiet").await;
    let repo = store.read().comments();
    assert_eq!(
        repo.approved_totals(item).await.unwrap(),
        ApprovedTotals::default()
    );

    let a = comment(item, None, CommentStatus::Approved, 0);
    let a1 = comment(item, Some(a.id), CommentStatus::Approved, 10);
    let a2 = comment(item, Some(a1.id), CommentStatus::Approved, 20);
    let trash = comment(item, None, CommentStatus::Trash, 30);
    let t1 = comment(item, Some(trash.id), CommentStatus::Approved, 40);
    let held = comment(item, None, CommentStatus::Pending, 50);
    let b = comment(item, None, CommentStatus::Approved, 60);
    insert_all(store, &[a, a1, a2, trash, t1, held, b]).await;
    insert_all(store, &[comment(quiet, None, CommentStatus::Pending, 0)]).await;

    assert_eq!(
        repo.approved_totals(item).await.unwrap(),
        ApprovedTotals {
            comments: 5,
            roots: 3
        }
    );
    assert_eq!(
        repo.approved_totals(quiet).await.unwrap(),
        ApprovedTotals::default()
    );
    assert_eq!(
        repo.approved_totals(ContentId::new()).await.unwrap(),
        ApprovedTotals::default()
    );
}
