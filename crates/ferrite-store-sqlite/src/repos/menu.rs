//! Menus and their item trees.
//!
//! `put` replaces a whole tree (delete items, upsert the menu row, insert items) and must
//! be atomic: see `atomic!` (own transaction on a pool, a savepoint inside a `Tx`).
//! Items are inserted parents-first (`ferrite_store::insertion_order`) rather than
//! deferring the `parent_id` foreign key, which SQLite only supports as DEFERRABLE.

use async_trait::async_trait;
use ferrite_domain::{Menu, MenuItem};
use ferrite_store::{MenuRepo, StoreError, insertion_order, tree_order, validate_text};
use sqlx::{Row, SqliteConnection};
use uuid::Uuid;

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
        // One statement, so the label and items come from the same snapshot.
        let rows = run!(&self.reader, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT m.label AS menu_label, {MENU_ITEM_COLUMNS} FROM menus m \
             LEFT JOIN menu_items i ON i.menu = m.name WHERE m.name = ?"
        )))
        .bind(name)
        .fetch_all(e))?;
        let Some(first) = rows.first() else {
            return Ok(None);
        };
        let label: String = first.try_get("menu_label").map_err(map_db_err)?;
        let mut items = Vec::with_capacity(rows.len());
        for row in &rows {
            // A menu without items yields one row whose item columns are NULL.
            let id: Option<Uuid> = row.try_get("id").map_err(map_db_err)?;
            if id.is_some() {
                items.push(codec::menu_item(row)?);
            }
        }
        Ok(Some(Menu {
            name: name.to_owned(),
            label,
            items: tree_order(items),
        }))
    }

    async fn put(&self, menu: &Menu) -> Result<(), StoreError> {
        validate_text("menu name", &menu.name)?;
        validate_text("menu label", &menu.label)?;
        for item in &menu.items {
            validate_text("menu item label", &item.label)?;
            if let Some(url) = &item.url {
                validate_text("menu item url", url)?;
            }
        }
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
