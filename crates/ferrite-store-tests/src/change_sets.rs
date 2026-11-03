//! Contract tests for the change-set tables of `0018_change_sets` (M6-5 task 1).

use ferrite_domain::{
    AgentKind, ChangeSetId, ChangeSetQuery, Image, ImageState, NewChange, NewChangeSet, Timestamp,
    UserId,
};
use ferrite_store::{Backend, Compat, Repos, StoreError};
use uuid::Uuid;

use crate::{TestDb, fixtures::fixture_user};

const MINUTE: i64 = 60 * 1_000_000;

fn plus(t: Timestamp, micros: i64) -> Timestamp {
    Timestamp::from_micros(t.as_micros() + micros).unwrap()
}

fn new_set(user: Option<UserId>, agent: Uuid, now: Timestamp) -> NewChangeSet {
    NewChangeSet {
        id: ChangeSetId::new(),
        user_id: user,
        agent_kind: AgentKind::OAuth,
        agent_id: agent,
        client: "Claude".into(),
        open_key: format!("oauth:{agent}"),
        now,
        idle_since: plus(now, -30 * MINUTE),
        max_changes: 500,
    }
}

fn change(set: ChangeSetId, seq: u32, at: Timestamp, before: Image, after: Image) -> NewChange {
    NewChange {
        set_id: set,
        seq,
        at,
        op: "content.update".into(),
        object_kind: "content".into(),
        object_id: format!("obj-{seq}"),
        label: "Hello".into(),
        before,
        after,
    }
}

fn present(hash: &str) -> Image {
    Image::Present {
        hash: hash.into(),
        bytes: vec![0x28, 0xb5, 0, 255],
    }
}

/// Opens (or extends) the set of `key` for `n` changes the way core does.
async fn reserve(r: &dyn Repos, s: &NewChangeSet, n: u32) -> (ChangeSetId, u32) {
    let cs = r.change_sets();
    match cs
        .extend_open(&s.open_key, s.now, s.idle_since, s.max_changes, n)
        .await
        .unwrap()
    {
        Some(x) => x,
        None => cs.open(s, n).await.unwrap(),
    }
}

fn query(now: Timestamp) -> ChangeSetQuery {
    ChangeSetQuery {
        user: None,
        agent_id: None,
        open: None,
        idle_since: plus(now, -30 * MINUTE),
        limit: 50,
        cursor: None,
    }
}

/// `0018_change_sets`: the tables exist; the down drops them; up again takes rows.
pub async fn migration_0018_roundtrip(db: &TestDb) {
    const TABLES: [&str; 2] = ["change_sets", "change_set_changes"];
    let store = db.store();
    let has =
        |objects: &[String], name: &str| objects.iter().any(|o| o.ends_with(&format!(":{name}")));
    let objects = db.user_objects().await;
    for name in TABLES {
        assert!(has(&objects, name), "{name}: {objects:?}");
    }
    assert!(has(&objects, "change_sets_agent_idx"), "{objects:?}");
    let now = Timestamp::now();
    let s = new_set(None, Uuid::now_v7(), now);
    reserve(store.read(), &s, 1).await;

    // Down to 0017 (a wave-B sibling's, or 0016 before the merge): every later migration
    // goes, 0018 among them, and comes back.
    let down = store.migrate_down(17, false).await.unwrap();
    assert!(down.contains(&18), "{down:?}");
    let objects = db.user_objects().await;
    for name in TABLES {
        assert!(!has(&objects, name), "{name} left: {objects:?}");
    }
    let mut up = down.clone();
    up.reverse();
    assert_eq!(store.migrate_up(None).await.unwrap().applied, up);
    assert_eq!(store.read().change_sets().get(s.id).await.unwrap(), None);
    let (id, seq) = reserve(store.read(), &s, 1).await;
    assert_eq!((id, seq), (s.id, 1));
    assert_eq!(
        store
            .migration_status(&crate::migrations::binary())
            .await
            .unwrap()
            .verdict,
        Compat::UpToDate
    );
}

pub async fn extend_open_returns_consecutive_seqs(db: &TestDb) {
    let r = db.store().read();
    let now = Timestamp::now();
    let s = new_set(None, Uuid::now_v7(), now);
    assert_eq!(reserve(r, &s, 1).await, (s.id, 1));
    assert_eq!(reserve(r, &s, 3).await, (s.id, 2));
    assert_eq!(reserve(r, &s, 1).await, (s.id, 5));
    let got = r.change_sets().get(s.id).await.unwrap().unwrap();
    assert_eq!(got.changes, 5);
    assert!(got.open);
    assert_eq!(got.client, "Claude");
    assert_eq!(got.agent_kind, AgentKind::OAuth);
}

pub async fn idle_set_is_not_extended(db: &TestDb) {
    let r = db.store().read();
    let t0 = Timestamp::now();
    let agent = Uuid::now_v7();
    let first = new_set(None, agent, t0);
    reserve(r, &first, 1).await;
    // 31 minutes later: the set is idle.
    let later = plus(t0, 31 * MINUTE);
    let second = new_set(None, agent, later);
    let cs = r.change_sets();
    assert_eq!(
        cs.extend_open(
            &second.open_key,
            later,
            second.idle_since,
            second.max_changes,
            1
        )
        .await
        .unwrap(),
        None
    );
    // 29 minutes after the last change: still open.
    let soon = plus(t0, 29 * MINUTE);
    assert_eq!(
        cs.extend_open(&first.open_key, soon, plus(soon, -30 * MINUTE), 500, 1)
            .await
            .unwrap(),
        Some((first.id, 2))
    );
}

pub async fn full_set_is_not_extended(db: &TestDb) {
    let r = db.store().read();
    let now = Timestamp::now();
    let mut s = new_set(None, Uuid::now_v7(), now);
    s.max_changes = 3;
    assert_eq!(reserve(r, &s, 3).await, (s.id, 1));
    let cs = r.change_sets();
    assert_eq!(
        cs.extend_open(&s.open_key, now, s.idle_since, 3, 1)
            .await
            .unwrap(),
        None
    );
    let mut next = new_set(None, s.agent_id, now);
    next.max_changes = 3;
    let (id, seq) = cs.open(&next, 1).await.unwrap();
    assert_eq!((id, seq), (next.id, 1));
    assert!(!cs.get(s.id).await.unwrap().unwrap().open);
}

pub async fn open_closes_the_stale_set_and_starts_a_new_one(db: &TestDb) {
    let r = db.store().read();
    let t0 = Timestamp::now();
    let agent = Uuid::now_v7();
    let first = new_set(None, agent, t0);
    reserve(r, &first, 2).await;
    let later = new_set(None, agent, plus(t0, 45 * MINUTE));
    assert_eq!(reserve(r, &later, 1).await, (later.id, 1));
    let cs = r.change_sets();
    let old = cs.get(first.id).await.unwrap().unwrap();
    assert!(!old.open);
    assert_eq!(old.changes, 2);
    assert!(cs.get(later.id).await.unwrap().unwrap().open);
    // A closed set is never extended again, even inside its old window.
    cs.close(later.id).await.unwrap();
    assert!(!cs.get(later.id).await.unwrap().unwrap().open);
    assert_eq!(
        cs.extend_open(&later.open_key, later.now, later.idle_since, 500, 1)
            .await
            .unwrap(),
        None
    );
}

/// Two transactions of one agent open at once: on Postgres they meet in one set through
/// the unique `open_key` (the second waits for the first's commit); SQLite serialises
/// writers, so they run one after the other.
pub async fn concurrent_open_meets_in_one_set(db: std::sync::Arc<TestDb>) {
    let store = db.store();
    let now = Timestamp::now();
    let agent = Uuid::now_v7();
    let a = new_set(None, agent, now);
    let b = new_set(None, agent, now);
    if store.backend() == Backend::Postgres {
        let tx_a = store.begin().await.unwrap();
        let tx_b = store.begin().await.unwrap();
        assert_eq!(
            tx_a.change_sets()
                .extend_open(&a.open_key, now, a.idle_since, 500, 1)
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            tx_b.change_sets()
                .extend_open(&b.open_key, now, b.idle_since, 500, 1)
                .await
                .unwrap(),
            None
        );
        let first = tx_a.change_sets().open(&a, 1).await.unwrap();
        let b_open = tokio::spawn(async move {
            let got = tx_b.change_sets().open(&b, 2).await.unwrap();
            tx_b.commit().await.unwrap();
            got
        });
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert!(!b_open.is_finished(), "the second opener waits");
        tx_a.commit().await.unwrap();
        let second = b_open.await.unwrap();
        assert_eq!(first, (a.id, 1));
        assert_eq!(second, (a.id, 2));
    } else {
        let tx = store.begin().await.unwrap();
        assert_eq!(reserve(&*tx, &a, 1).await, (a.id, 1));
        tx.commit().await.unwrap();
        let tx = store.begin().await.unwrap();
        assert_eq!(reserve(&*tx, &b, 2).await, (a.id, 2));
        tx.commit().await.unwrap();
    }
    let got = store.read().change_sets().get(a.id).await.unwrap().unwrap();
    assert_eq!(got.changes, 3);
}

pub async fn images_round_trip_bytes_and_hashes(db: &TestDb) {
    let r = db.store().read();
    let now = Timestamp::now();
    let s = new_set(None, Uuid::now_v7(), now);
    let (id, _) = reserve(r, &s, 3).await;
    let cs = r.change_sets();
    let c1 = change(id, 1, now, Image::Absent, present("h1"));
    let c2 = change(
        id,
        2,
        now,
        present("h1"),
        Image::Skipped { hash: "h2".into() },
    );
    let c3 = change(
        id,
        3,
        now,
        Image::Skipped { hash: "h2".into() },
        Image::Absent,
    );
    for c in [&c1, &c2, &c3] {
        cs.insert_change(c).await.unwrap();
    }
    assert!(matches!(
        cs.insert_change(&c1).await,
        Err(StoreError::UniqueViolation { .. })
    ));
    let mut bad = c1.clone();
    bad.set_id = ChangeSetId::new();
    assert!(matches!(
        cs.insert_change(&bad).await,
        Err(StoreError::ForeignKey { .. })
    ));
    let mut nul = c1.clone();
    nul.seq = 9;
    nul.label = "a\0b".into();
    assert!(matches!(
        cs.insert_change(&nul).await,
        Err(StoreError::Invalid(_))
    ));

    let got = cs.change(id, 2).await.unwrap().unwrap();
    assert_eq!(got.before, c2.before);
    assert_eq!(got.after, c2.after);
    assert_eq!(got.label, "Hello");
    assert!(got.reverted_at.is_none() && !got.revert_forced);
    assert_eq!(cs.change(id, 7).await.unwrap(), None);
    let all = cs.changes(id).await.unwrap();
    assert_eq!(all.iter().map(|c| c.seq).collect::<Vec<_>>(), [1, 2, 3]);
    assert_eq!(all[0].before, Image::Absent);
    assert_eq!(all[0].after, c1.after);
    let sums = cs.summaries(id).await.unwrap();
    let states: Vec<_> = sums
        .iter()
        .map(|c| (c.before_state, c.after_state))
        .collect();
    assert_eq!(
        states,
        [
            (ImageState::Absent, ImageState::Present),
            (ImageState::Present, ImageState::Skipped),
            (ImageState::Skipped, ImageState::Absent),
        ]
    );
    assert_eq!(sums[1].before_hash.as_deref(), Some("h1"));
    assert_eq!(sums[1].after_hash.as_deref(), Some("h2"));
}

pub async fn list_is_newest_first_with_cursor_and_filters(db: &TestDb) {
    let store = db.store();
    let r = store.read();
    let user = fixture_user(store).await;
    let now = Timestamp::now();
    let agent = Uuid::now_v7();
    let mut ids = Vec::new();
    for i in 0..3 {
        let s = new_set(Some(user.id), Uuid::now_v7(), plus(now, -(i + 1) * MINUTE));
        reserve(r, &s, 1).await;
        ids.push(s.id);
    }
    // Another user's, the newest, from a known agent; stale (an hour old) is "closed".
    let mine = new_set(None, agent, now);
    reserve(r, &mine, 1).await;
    let old = new_set(None, Uuid::now_v7(), plus(now, -60 * MINUTE));
    reserve(r, &old, 1).await;

    let cs = r.change_sets();
    let all = cs.list(&query(now)).await.unwrap();
    assert_eq!(all.items.len(), 5);
    assert_eq!(all.items[0].id, mine.id);
    let mut q = query(now);
    q.user = Some(user.id);
    q.limit = 2;
    let p1 = cs.list(&q).await.unwrap();
    assert_eq!(
        p1.items.iter().map(|s| s.id).collect::<Vec<_>>(),
        ids[..2].to_vec()
    );
    q.cursor = p1.next_cursor;
    let p2 = cs.list(&q).await.unwrap();
    assert_eq!(
        p2.items.iter().map(|s| s.id).collect::<Vec<_>>(),
        ids[2..].to_vec()
    );
    assert_eq!(p2.next_cursor, None);

    let mut q = query(now);
    q.agent_id = Some(agent);
    assert_eq!(
        cs.list(&q)
            .await
            .unwrap()
            .items
            .iter()
            .map(|s| s.id)
            .collect::<Vec<_>>(),
        [mine.id]
    );
    let mut q = query(now);
    q.open = Some(false);
    assert_eq!(
        cs.list(&q)
            .await
            .unwrap()
            .items
            .iter()
            .map(|s| s.id)
            .collect::<Vec<_>>(),
        [old.id]
    );
    q.open = Some(true);
    assert_eq!(cs.list(&q).await.unwrap().items.len(), 4);
    let mut q = query(now);
    q.cursor = Some("nonsense".into());
    assert!(cs.list(&q).await.is_err());
}

pub async fn reverted_counts_in_list(db: &TestDb) {
    let store = db.store();
    let r = store.read();
    let user = fixture_user(store).await;
    let now = Timestamp::now();
    let s = new_set(Some(user.id), Uuid::now_v7(), now);
    let (id, _) = reserve(r, &s, 3).await;
    let cs = r.change_sets();
    for seq in 1..=3 {
        cs.insert_change(&change(id, seq, now, present("a"), present("b")))
            .await
            .unwrap();
    }
    cs.mark_reverted(id, &[3, 2], user.id, now, &[2])
        .await
        .unwrap();
    let got = cs.get(id).await.unwrap().unwrap();
    assert_eq!(got.reverted_changes, 2);
    assert_eq!(got.reverted_at, None, "one change is left");
    let c2 = cs.change(id, 2).await.unwrap().unwrap();
    assert!(c2.revert_forced);
    assert_eq!(c2.reverted_by, Some(user.id));
    assert!(!cs.change(id, 3).await.unwrap().unwrap().revert_forced);
    assert!(
        cs.change(id, 1)
            .await
            .unwrap()
            .unwrap()
            .reverted_at
            .is_none()
    );
    cs.mark_reverted(id, &[1], user.id, now, &[]).await.unwrap();
    let got = cs.get(id).await.unwrap().unwrap();
    assert_eq!(got.reverted_changes, 3);
    assert_eq!(got.reverted_by, Some(user.id));
    assert!(got.reverted_at.is_some());
    let listed = cs.list(&query(now)).await.unwrap();
    assert_eq!(listed.items[0].reverted_changes, 3);
    assert_eq!(
        cs.changes_since(Some(user.id), plus(now, -MINUTE))
            .await
            .unwrap(),
        3
    );
    assert_eq!(cs.changes_since(None, plus(now, MINUTE)).await.unwrap(), 0);
    assert_eq!(
        cs.changes_since(Some(UserId::new()), plus(now, -MINUTE))
            .await
            .unwrap(),
        0
    );
}

pub async fn deleting_user_nulls_user_and_reverted_by(db: &TestDb) {
    let store = db.store();
    let r = store.read();
    let user = fixture_user(store).await;
    let now = Timestamp::now();
    let s = new_set(Some(user.id), Uuid::now_v7(), now);
    let (id, _) = reserve(r, &s, 1).await;
    let cs = r.change_sets();
    cs.insert_change(&change(id, 1, now, Image::Absent, present("a")))
        .await
        .unwrap();
    cs.mark_reverted(id, &[1], user.id, now, &[]).await.unwrap();
    r.users().delete(user.id).await.unwrap();
    let got = cs.get(id).await.unwrap().unwrap();
    assert_eq!((got.user_id, got.reverted_by), (None, None));
    assert_eq!(cs.change(id, 1).await.unwrap().unwrap().reverted_by, None);
}

pub async fn prune_deletes_old_sets_and_their_changes(db: &TestDb) {
    let r = db.store().read();
    let now = Timestamp::now();
    let day = 24 * 60 * MINUTE;
    let old = new_set(None, Uuid::now_v7(), plus(now, -100 * day));
    let (old_id, _) = reserve(r, &old, 1).await;
    let fresh = new_set(None, Uuid::now_v7(), now);
    let (fresh_id, _) = reserve(r, &fresh, 1).await;
    let cs = r.change_sets();
    cs.insert_change(&change(old_id, 1, old.now, Image::Absent, present("a")))
        .await
        .unwrap();
    assert!(cs.prune(now, 0).await.is_err());
    assert_eq!(cs.prune(plus(now, -90 * day), 100).await.unwrap(), 1);
    assert_eq!(cs.get(old_id).await.unwrap(), None);
    assert_eq!(cs.change(old_id, 1).await.unwrap(), None);
    assert!(cs.get(fresh_id).await.unwrap().is_some());
}
