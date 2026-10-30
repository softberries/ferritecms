//! Contract tests for [`Store::pool_stats`](ferrite_store::Store::pool_stats) (M4-2
//! P34, `ferrite_db_pool_connections`): an open transaction is a used connection, and
//! it is idle again once the transaction ends.

use std::time::Duration;

use tokio::time::Instant;

use crate::TestDb;

pub async fn pool_stats_count_used_and_idle(db: &TestDb) {
    let store = db.store();
    // A read leaves at least one pooled connection behind.
    store.read().options().get("site.title").await.unwrap();
    let tx = store.begin().await.unwrap();
    let held = store.pool_stats();
    assert!(
        held.used >= 1,
        "the transaction's connection is used: {held:?}"
    );
    tx.commit().await.unwrap();
    // A connection goes back to its pool asynchronously.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let stats = store.pool_stats();
        if stats.used == 0 {
            assert!(stats.idle >= 1, "back in the pool: {stats:?}");
            break;
        }
        assert!(Instant::now() < deadline, "not released: {stats:?}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
