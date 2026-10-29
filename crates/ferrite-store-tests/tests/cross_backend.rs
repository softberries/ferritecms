//! A site moved SQLite -> Postgres -> SQLite through the export reader and the loader
//! comes back row for row (M4-1 P1, P4). Needs `FERRITE_TEST_PG_URL` (else SKIP).

use ferrite_store_tests::{
    TestDb,
    import_writer::{add_repo_rows, assert_same, dump_all, load_all, rich_site},
};

#[tokio::test]
async fn cross_backend_round_trip_is_identical() {
    let Some(pg) = TestDb::postgres().await else {
        return;
    };
    let source = TestDb::sqlite().await;
    load_all(source.store(), &rich_site(), 4).await;
    add_repo_rows(source.store()).await;
    let from_sqlite = dump_all(source.store(), 5).await;

    load_all(pg.store(), &from_sqlite, 3).await;
    let from_postgres = dump_all(pg.store(), 7).await;
    assert_same(&from_sqlite, &from_postgres);

    let back = TestDb::sqlite().await;
    load_all(back.store(), &from_postgres, 500).await;
    let from_sqlite_again = dump_all(back.store(), 1000).await;
    assert_same(&from_sqlite, &from_sqlite_again);

    // Every table serializes to the same bytes from both backends.
    for ((name, a), (_, b)) in from_sqlite.iter().zip(&from_postgres) {
        assert_eq!(
            serde_json::to_string(a).unwrap(),
            serde_json::to_string(b).unwrap(),
            "{name}"
        );
    }

    back.teardown().await;
    source.teardown().await;
    pg.teardown().await;
}
