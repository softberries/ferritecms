//! `change_sets.prune` (M6-5, design 17 §12): sets older than `mcp.change_sets_days`
//! (default 90) go, with their changes.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use ferrite_cluster::{JobHandler, builtin::ChangeSetsPrune};
use ferrite_domain::{
    AgentKind, CHANGE_SETS_DAYS_OPTION, ChangeSetId, Job, JobId, JobStatus, NewChangeSet, Timestamp,
};
use ferrite_store_tests::{TestDb, backend_tests};
use serde_json::json;

const DAY: i64 = 24 * 3600 * 1_000_000;

fn ago(days: i64) -> Timestamp {
    Timestamp::from_micros(Timestamp::now().as_micros() - days * DAY).unwrap()
}

async fn set_at(db: &TestDb, at: Timestamp) -> ChangeSetId {
    let agent = ferrite_domain::ApiTokenId::new().as_uuid();
    let (id, _) = db
        .store()
        .read()
        .change_sets()
        .open(
            &NewChangeSet {
                id: ChangeSetId::new(),
                user_id: None,
                agent_kind: AgentKind::Token,
                agent_id: agent,
                client: "test".into(),
                open_key: format!("token:{agent}"),
                now: at,
                idle_since: at,
                max_changes: 500,
            },
            1,
        )
        .await
        .unwrap();
    id
}

fn job() -> Job {
    let now = Timestamp::now();
    Job {
        id: JobId::new(),
        kind: "change_sets.prune".into(),
        payload: json!({"v": 1}),
        unique_key: None,
        run_at: now,
        priority: 0,
        status: JobStatus::Running,
        attempts: 1,
        max_attempts: 3,
        locked_by: None,
        locked_until: None,
        last_error: None,
        created_at: now,
        finished_at: None,
    }
}

backend_tests!(prune_respects_change_sets_days_and_default);

async fn prune_respects_change_sets_days_and_default(db: &TestDb) {
    let old = set_at(db, ago(100)).await;
    let middle = set_at(db, ago(40)).await;
    let fresh = set_at(db, ago(1)).await;
    let handler = ChangeSetsPrune(db.shared_store());
    handler.run(&job()).await.unwrap();
    let cs = db.store().read().change_sets();
    assert!(
        cs.get(old).await.unwrap().is_none(),
        "older than the default 90 days"
    );
    assert!(cs.get(middle).await.unwrap().is_some());
    db.store()
        .read()
        .options()
        .set(CHANGE_SETS_DAYS_OPTION, &json!(30), true)
        .await
        .unwrap();
    handler.run(&job()).await.unwrap();
    assert!(
        cs.get(middle).await.unwrap().is_none(),
        "older than 30 days"
    );
    assert!(cs.get(fresh).await.unwrap().is_some());
}
