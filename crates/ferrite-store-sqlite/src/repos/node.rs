//! The node registry (04 §3, P14) on SQLite. The process clock is the database clock
//! (one process owns the file), so `now` is bound from `Timestamp::now()`.

use async_trait::async_trait;
use ferrite_domain::{NewNode, Node, NodeId, Timestamp};
use ferrite_store::{MAX_NODES, NodeRepo, StoreError, check_new_node, check_node_meta};
use sqlx::AssertSqlSafe;

use super::SqliteRepos;
use crate::{
    codec::{self, NODE_COLUMNS},
    exec::run,
};

#[async_trait]
impl NodeRepo for SqliteRepos {
    async fn register(&self, node: &NewNode) -> Result<(), StoreError> {
        check_new_node(node)?;
        let meta = codec::json_to_db(&node.meta)?;
        let now = Timestamp::now().as_micros();
        run!(&self.writer, |e| sqlx::query(
            "INSERT INTO nodes (id, name, version, schema_version, started_at, heartbeat_at, \
             meta) VALUES (?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(node.id.as_uuid())
        .bind(&node.name)
        .bind(&node.version)
        .bind(node.schema_version)
        .bind(now)
        .bind(now)
        .bind(meta)
        .execute(e))?;
        Ok(())
    }

    async fn heartbeat(&self, id: NodeId, meta: &serde_json::Value) -> Result<bool, StoreError> {
        check_node_meta(meta)?;
        let meta = codec::json_to_db(meta)?;
        let done = run!(&self.writer, |e| sqlx::query(
            "UPDATE nodes SET heartbeat_at = ?, meta = ? WHERE id = ?"
        )
        .bind(Timestamp::now().as_micros())
        .bind(meta)
        .bind(id.as_uuid())
        .execute(e))?;
        Ok(done.rows_affected() > 0)
    }

    async fn deregister(&self, id: NodeId) -> Result<(), StoreError> {
        run!(&self.writer, |e| sqlx::query(
            "DELETE FROM nodes WHERE id = ?"
        )
        .bind(id.as_uuid())
        .execute(e))?;
        Ok(())
    }

    async fn list(&self) -> Result<Vec<Node>, StoreError> {
        let rows = run!(&self.reader, |e| sqlx::query(AssertSqlSafe(format!(
            "SELECT {NODE_COLUMNS} FROM nodes ORDER BY started_at, id LIMIT ?"
        )))
        .bind(i64::from(MAX_NODES))
        .fetch_all(e))?;
        rows.iter().map(codec::node).collect()
    }

    async fn db_now(&self) -> Result<Timestamp, StoreError> {
        Ok(Timestamp::now())
    }

    async fn prune(&self, heartbeat_before: Timestamp) -> Result<u64, StoreError> {
        let done = run!(&self.writer, |e| sqlx::query(
            "DELETE FROM nodes WHERE heartbeat_at < ?"
        )
        .bind(heartbeat_before.as_micros())
        .execute(e))?;
        Ok(done.rows_affected())
    }
}
