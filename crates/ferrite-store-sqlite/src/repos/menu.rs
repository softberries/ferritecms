//! Menus and their item trees.
//!
//! `put` replaces a whole tree (delete items, upsert the menu row, insert items) and must
//! be atomic: see `atomic!` (own transaction on a pool, a savepoint inside a `Tx`).
//! Items are inserted parents-first (`ferrite_store::insertion_order`) rather than
//! deferring the `parent_id` foreign key, which SQLite only supports as DEFERRABLE.

use async_trait::async_trait;
use ferrite_domain::{Menu, MenuItem};
use ferrite_store::{MenuRepo, StoreError, insertion_order, tree_order};
use sqlx::SqliteConnection;

use super::SqliteRepos;
use crate::{
    codec::{self, MENU_ITEM_COLUMNS},
    exec::{atomic, map_db_err, run},
};

async fn replace_menu(
    conn: &mut SqliteConnection,
    menu: &Menu,
    items: &[&MenuItem],
) -> Result<(), StoreError> {
    sqlx::query(
        "INSERT INTO menus (name, label) VALUES (?, ?) \
         ON CONFLICT (name) DO UPDATE SET label = excluded.label",
    )
    .bind(&menu.name)
    .bind(&menu.label)
    .execute(&mut *conn)
    .await
    .map_err(map_db_err)?;
    sqlx::query("DELETE FROM menu_items WHERE menu = ?")
        .bind(&menu.name)
        .execute(&mut *conn)
        .await
        .map_err(map_db_err)?;
    for item in items {
        sqlx::query(
            "INSERT INTO menu_items (id, menu, parent_id, position, kind, target_id, url, label) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(item.id.as_uuid())
        .bind(&menu.name)
        .bind(item.parent_id.map(|p| p.as_uuid()))
        .bind(item.position)
        .bind(item.kind.as_str())
        .bind(item.target_id)
        .bind(&item.url)
        .bind(&item.label)
        .execute(&mut *conn)
        .await
        .map_err(map_db_err)?;
    }
    Ok(())
}

#[async_trait]
impl MenuRepo for SqliteRepos {
    async fn get(&self, name: &str) -> Result<Option<Menu>, StoreError> {
        let label: Option<String> = run!(&self.reader, |e| sqlx::query_scalar(
            "SELECT label FROM menus WHERE name = ?"
        )
        .bind(name)
        .fetch_optional(e))?;
        let Some(label) = label else {
            return Ok(None);
        };
        let rows = run!(&self.reader, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {MENU_ITEM_COLUMNS} FROM menu_items WHERE menu = ?"
        )))
        .bind(name)
        .fetch_all(e))?;
        let items = rows
            .iter()
            .map(codec::menu_item)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Some(Menu {
            name: name.to_owned(),
            label,
            items: tree_order(items),
        }))
    }

    async fn put(&self, menu: &Menu) -> Result<(), StoreError> {
        let items = insertion_order(menu)?;
        atomic!(&self.writer, |conn| replace_menu(conn, menu, &items))
    }

    async fn list(&self) -> Result<Vec<(String, String)>, StoreError> {
        run!(&self.reader, |e| sqlx::query_as(
            "SELECT name, label FROM menus ORDER BY name"
        )
        .fetch_all(e))
    }

    async fn delete(&self, name: &str) -> Result<(), StoreError> {
        // Items cascade.
        run!(&self.writer, |e| sqlx::query(
            "DELETE FROM menus WHERE name = ?"
        )
        .bind(name)
        .execute(e))
        .map(|_| ())
    }
}
