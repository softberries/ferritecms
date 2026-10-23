//! The node registry (04 §3, P14) on Postgres: every time is the database clock (`now()`).

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ferrite_domain::{NewNode, Node, NodeId, Timestamp};
use ferrite_store::{
    MAX_NODES, NodeRepo, StoreError, check_new_node, check_node_meta, check_prune_limit,
};
use sqlx::{AssertSqlSafe, types::Json};

use super::PgRepos;
use crate::{
    codec::{self, NODE_COLUMNS},
    exec::run,
};

#[async_trait]
impl NodeRepo for PgRepos {
    async fn register(&self, node: &NewNode) -> Result<(), StoreError> {
        check_new_node(node)?;
        run!(&self.exec, |e| sqlx::query(
            "INSERT INTO nodes (id, name, version, schema_version, started_at, heartbeat_at, \
             meta) VALUES ($1, $2, $3, $4, now(), now(), $5)"
        )
        .bind(node.id.as_uuid())
        .bind(&node.name)
        .bind(&node.version)
        .bind(node.schema_version)
        .bind(Json(&node.meta))
        .execute(e))?;
        Ok(())
    }

    async fn heartbeat(&self, id: NodeId, meta: &serde_json::Value) -> Result<bool, StoreError> {
        check_node_meta(meta)?;
        let done = run!(&self.exec, |e| sqlx::query(
            "UPDATE nodes SET heartbeat_at = now(), meta = $2 WHERE id = $1"
        )
        .bind(id.as_uuid())
        .bind(Json(meta))
        .execute(e))?;
        Ok(done.rows_affected() > 0)
    }

    async fn deregister(&self, id: NodeId) -> Result<(), StoreError> {
        run!(&self.exec, |e| sqlx::query(
            "DELETE FROM nodes WHERE id = $1"
        )
        .bind(id.as_uuid())
        .execute(e))?;
        Ok(())
    }

    async fn list(&self) -> Result<Vec<Node>, StoreError> {
        let rows = run!(&self.exec, |e| sqlx::query(AssertSqlSafe(format!(
            "SELECT {NODE_COLUMNS} FROM nodes ORDER BY started_at, id LIMIT $1"
        )))
        .bind(i64::from(MAX_NODES))
        .fetch_all(e))?;
        rows.iter().map(codec::node).collect()
    }

    async fn db_now(&self) -> Result<Timestamp, StoreError> {
        let now: DateTime<Utc> = run!(&self.exec, |e| sqlx::query_scalar("SELECT now()")
            .fetch_one(e))?;
        Ok(now.into())
    }

    async fn prune(&self, heartbeat_before: Timestamp, limit: u32) -> Result<u64, StoreError> {
        let limit = check_prune_limit(limit)?;
        let done = run!(&self.exec, |e| sqlx::query(
            "DELETE FROM nodes WHERE id IN \
             (SELECT id FROM nodes WHERE heartbeat_at < $1 LIMIT $2)"
        )
        .bind(DateTime::<Utc>::from(heartbeat_before))
        .bind(limit)
        .execute(e))?;
        Ok(done.rows_affected())
    }
}
