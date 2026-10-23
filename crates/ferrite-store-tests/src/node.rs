//! Contract tests for the node registry (04 §3, P14).

use std::time::Duration;

use ferrite_domain::{NewNode, Node, NodeId, Timestamp};
use ferrite_store::{NodeRepo, StoreError};
use serde_json::json;

use crate::TestDb;

fn new_node(name: &str) -> NewNode {
    NewNode {
        id: NodeId::new(),
        name: name.into(),
        version: "0.1.0".into(),
        schema_version: 4,
        meta: json!({"mode": "solo", "name": name}),
    }
}

async fn by_id(nodes: &dyn NodeRepo, id: NodeId) -> Node {
    nodes
        .list()
        .await
        .unwrap()
        .into_iter()
        .find(|n| n.id == id)
        .expect("node listed")
}

pub async fn nodes_register_heartbeat_list_prune(db: &TestDb) {
    let nodes = db.store().read().nodes();

    let local = Timestamp::now();
    let now = nodes.db_now().await.unwrap();
    let skew = (now.as_micros() - local.as_micros()).abs();
    assert!(
        skew <= 2_000_000,
        "db_now is {skew} µs from the local clock"
    );

    let a = new_node("a");
    let b = new_node("b");
    nodes.register(&a).await.unwrap();
    tokio::time::sleep(Duration::from_millis(5)).await;
    nodes.register(&b).await.unwrap();

    let listed = nodes.list().await.unwrap();
    assert_eq!(
        listed.iter().map(|n| n.id).collect::<Vec<_>>(),
        [a.id, b.id],
        "ordered by started_at"
    );
    let first = &listed[0];
    assert_eq!(
        (
            &first.name,
            &first.version,
            first.schema_version,
            &first.meta
        ),
        (&a.name, &a.version, a.schema_version, &a.meta)
    );
    assert_eq!(first.started_at, first.heartbeat_at);
    assert!(listed[0].started_at <= listed[1].started_at);

    assert!(matches!(
        nodes.register(&a).await,
        Err(StoreError::UniqueViolation { constraint }) if constraint == "nodes_pkey"
    ));

    tokio::time::sleep(Duration::from_millis(20)).await;
    let meta = json!({"mode": "cluster", "theme": "blog"});
    assert!(nodes.heartbeat(a.id, &meta).await.unwrap());
    let beat = by_id(nodes, a.id).await;
    assert!(
        beat.heartbeat_at > first.heartbeat_at,
        "heartbeat moves forward"
    );
    assert_eq!(beat.started_at, first.started_at);
    assert_eq!(beat.meta, meta, "meta replaced");
    assert!(
        !nodes.heartbeat(NodeId::new(), &meta).await.unwrap(),
        "missing row"
    );

    // Validation, before any SQL.
    let mut bad = new_node("c");
    bad.meta = json!([1]);
    assert!(matches!(
        nodes.register(&bad).await,
        Err(StoreError::Invalid(_))
    ));
    let mut bad = new_node("c\0");
    assert!(matches!(
        nodes.register(&bad).await,
        Err(StoreError::Invalid(_))
    ));
    bad.name = "c".into();
    bad.version = "\0".into();
    assert!(matches!(
        nodes.register(&bad).await,
        Err(StoreError::Invalid(_))
    ));
    for meta in [json!("x"), json!({"k": "\0"})] {
        assert!(matches!(
            nodes.heartbeat(a.id, &meta).await,
            Err(StoreError::Invalid(_))
        ));
    }

    // b last beat at registration, before a's heartbeat: only b is older than the cut.
    assert_eq!(nodes.prune(beat.heartbeat_at, 100).await.unwrap(), 1);
    assert_eq!(
        nodes
            .list()
            .await
            .unwrap()
            .iter()
            .map(|n| n.id)
            .collect::<Vec<_>>(),
        [a.id]
    );
    assert_eq!(nodes.prune(beat.heartbeat_at, 100).await.unwrap(), 0);

    nodes.deregister(a.id).await.unwrap();
    assert!(nodes.list().await.unwrap().is_empty());
    nodes.deregister(a.id).await.unwrap();
    assert!(!nodes.heartbeat(a.id, &meta).await.unwrap(), "deregistered");
}

/// R5: `prune` deletes at most `limit` nodes per call; the limit is checked.
pub async fn nodes_prune_in_batches(db: &TestDb) {
    let nodes = db.store().read().nodes();
    for i in 0..3 {
        nodes.register(&new_node(&format!("n{i}"))).await.unwrap();
    }
    let future = Timestamp::from_micros(Timestamp::now().as_micros() + 1_000_000).unwrap();
    for bad in [0, ferrite_store::MAX_PRUNE_BATCH + 1] {
        assert!(matches!(
            nodes.prune(future, bad).await,
            Err(StoreError::Invalid(_))
        ));
    }
    assert_eq!(nodes.prune(future, 2).await.unwrap(), 2);
    assert_eq!(nodes.list().await.unwrap().len(), 1);
    assert_eq!(nodes.prune(future, 2).await.unwrap(), 1);
    assert_eq!(nodes.prune(future, 2).await.unwrap(), 0);
}
