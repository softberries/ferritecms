//! The single-transaction row loader (M4-1 P4): writes neutral [`Row`]s from an export
//! into a database, all in one write transaction.
//!
//! The backend-neutral part lives here: [`Loader`] checks every row against the
//! catalogue, decodes it, orders self-referencing rows and cuts them into multi-row
//! insert batches; a backend only supplies a [`LoadTarget`] that runs the statements.

use std::collections::{HashMap, HashSet};

use async_trait::async_trait;
use uuid::Uuid;

use crate::{
    StoreError,
    catalogue::{self, Cell, ColType, Export, Row, TableSpec, decode_column},
};

/// Most rows in one multi-row `INSERT`.
pub const MAX_LOAD_BATCH: usize = 500;

/// Loads rows into one write transaction, opened by
/// [`Store::import_writer`](crate::Store::import_writer) and made durable only by
/// [`commit`](Self::commit). Dropping the writer without `commit` rolls everything back
/// (Postgres: the dedicated connection is closed).
///
/// Any failed call (a refused row or a database error) poisons the writer: every later
/// call returns [`StoreError::Aborted`], and `commit` rolls back and returns it, so a
/// load lands completely or not at all.
///
/// Tables are loaded in catalogue order ([`catalogue::TABLES`]): the foreign keys of a
/// row must already be loaded, except the table's own parent column
/// ([`TableSpec::self_ref`]). A row whose parent is not loaded yet waits in memory and is
/// inserted right after its parent (never with a placeholder parent: `content_items`'
/// slug is unique per parent, so placeholders would collide, ruling R6), so the rows of a
/// tree may come in any order.
///
/// Every parent must be part of the same load (ruling R8: the target is fresh). A row
/// whose parent is in the target's database but not in this load, never comes, or
/// lies on a cycle (including a row that is its own parent) keeps waiting, and
/// [`finish_table`](Self::finish_table) and [`commit`](Self::commit) refuse it.
///
/// Memory: rows that wait are held (decoded) until their parent arrives. With an export
/// paged by key (random or time-ordered UUIDs), that can be about half of the non-root
/// rows of a tree table (`content_items`, `terms`, `comments`, `menu_items`); in
/// addition the key (16 bytes plus hash-set overhead) of every loaded row of those
/// tables is kept until the writer ends.
#[async_trait]
pub trait ImportWriter: Send {
    /// Deletes every row of `tables` (in reverse foreign-key order, whatever the order
    /// given); for an [`OnlyKeys`](Export::OnlyKeys) table only the rows with those keys.
    /// Used on a fresh target for the rows the migrations seed (built-in content types
    /// and taxonomies). Each table must be the catalogue's own entry and exported.
    async fn clear(&mut self, tables: &[&TableSpec]) -> Result<(), StoreError>;

    /// Inserts `rows` of `table` (the catalogue's own entry, exported). Each row writes
    /// exactly the columns it holds, so an archive of an older schema loads (P4); every
    /// key must be a column of the table (else `Invalid`, before any SQL) and every value
    /// in the neutral encoding of the column's type. A missing `NOT NULL` column fails in
    /// the database. Rows of an [`OnlyKeys`](Export::OnlyKeys) table must have one of
    /// those keys and are written over the target's rows (`schema_meta`: `site_id`).
    async fn rows(&mut self, table: &TableSpec, rows: Vec<Row>) -> Result<(), StoreError>;

    /// Ends `table`: `Invalid` if any of its rows still waits for a parent that was not
    /// loaded.
    async fn finish_table(&mut self, table: &TableSpec) -> Result<(), StoreError>;

    /// Commits the load (after the [`finish_table`](Self::finish_table) check of every
    /// table).
    async fn commit(self: Box<Self>) -> Result<(), StoreError>;
}

/// One multi-row `INSERT`: the columns (a subset of the table's, in catalogue order) and
/// the decoded values of each row in that order.
#[derive(Debug, Clone, PartialEq)]
pub struct InsertBatch {
    pub columns: Vec<(&'static str, ColType)>,
    pub rows: Vec<Vec<Cell>>,
}

/// The statements a backend runs for a [`Loader`], all inside its one transaction.
#[async_trait]
pub trait LoadTarget: Send {
    /// Most bind parameters one statement may have (SQLite 32 766, Postgres 65 535).
    fn max_params(&self) -> usize;

    /// Runs one `INSERT` of `batch` into `table`; with `upsert`, a row whose key exists
    /// overwrites the other columns ([`insert_sql`] builds the statement).
    async fn insert(
        &mut self,
        table: &'static TableSpec,
        batch: InsertBatch,
        upsert: bool,
    ) -> Result<(), StoreError>;

    /// Deletes every row of `table`, or with `keys` only the rows whose (single-column)
    /// key is one of them ([`delete_sql`]).
    async fn delete(
        &mut self,
        table: &'static TableSpec,
        keys: Option<&'static [&'static str]>,
    ) -> Result<(), StoreError>;

    /// Commits the transaction.
    async fn commit(self) -> Result<(), StoreError>;
}

/// The rows of one self-referencing table seen so far.
#[derive(Default)]
struct Tree {
    /// Keys of the rows inserted (or queued for insertion) by this load.
    loaded: HashSet<Uuid>,
    /// Rows whose parent is not loaded yet, by parent key.
    waiting: HashMap<Uuid, Vec<Prepared>>,
}

impl Tree {
    fn waiting_rows(&self) -> usize {
        self.waiting.values().map(Vec::len).sum()
    }
}

/// One checked, decoded row.
struct Prepared {
    columns: Vec<(&'static str, ColType)>,
    cells: Vec<Cell>,
}

/// The backend-neutral [`ImportWriter`] over a backend's [`LoadTarget`]: checks and
/// decodes rows, orders tree rows parent first (holding waiting rows in memory, see
/// [`ImportWriter`] for the cost and the same-load rule), batches and poisons.
pub struct Loader<T> {
    target: T,
    poisoned: bool,
    trees: HashMap<&'static str, Tree>,
}

impl<T: LoadTarget> Loader<T> {
    /// A loader writing through `target` (whose transaction is already open).
    pub fn new(target: T) -> Self {
        Self {
            target,
            poisoned: false,
            trees: HashMap::new(),
        }
    }

    async fn try_clear(&mut self, tables: &[&TableSpec]) -> Result<(), StoreError> {
        let mut own = tables
            .iter()
            .map(|t| loadable(t))
            .collect::<Result<Vec<_>, _>>()?;
        own.sort_by_key(|t| std::cmp::Reverse(position(t)));
        own.dedup_by_key(|t| t.name);
        for table in own {
            let keys = match table.export {
                Export::OnlyKeys(keys) => Some(keys),
                _ => None,
            };
            self.target.delete(table, keys).await?;
        }
        Ok(())
    }

    async fn try_rows(&mut self, table: &TableSpec, rows: Vec<Row>) -> Result<(), StoreError> {
        let table = loadable(table)?;
        let mut prepared = rows
            .iter()
            .map(|row| prepare(table, row))
            .collect::<Result<Vec<_>, _>>()?;
        if let Some(parent_col) = table.self_ref {
            let tree = self.trees.entry(table.name).or_default();
            prepared = order_tree(table, parent_col, tree, prepared)?;
        }
        let upsert = matches!(table.export, Export::OnlyKeys(_));
        for batch in batches(prepared, self.target.max_params()) {
            self.target.insert(table, batch, upsert).await?;
        }
        Ok(())
    }

    fn check_finished(&self, table: &TableSpec) -> Result<(), StoreError> {
        let Some(tree) = self.trees.get(table.name) else {
            return Ok(());
        };
        let Some(parent) = tree.waiting.keys().min() else {
            return Ok(());
        };
        Err(StoreError::Invalid(format!(
            "{}: {} rows wait for a parent that is not in the load (e.g. {parent})",
            table.name,
            tree.waiting_rows()
        )))
    }

    /// Poisons the loader when `result` failed.
    fn guard(&mut self, result: Result<(), StoreError>) -> Result<(), StoreError> {
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }
}

#[async_trait]
impl<T: LoadTarget> ImportWriter for Loader<T> {
    async fn clear(&mut self, tables: &[&TableSpec]) -> Result<(), StoreError> {
        if self.poisoned {
            return Err(StoreError::Aborted);
        }
        let result = self.try_clear(tables).await;
        self.guard(result)
    }

    async fn rows(&mut self, table: &TableSpec, rows: Vec<Row>) -> Result<(), StoreError> {
        if self.poisoned {
            return Err(StoreError::Aborted);
        }
        let result = self.try_rows(table, rows).await;
        self.guard(result)
    }

    async fn finish_table(&mut self, table: &TableSpec) -> Result<(), StoreError> {
        if self.poisoned {
            return Err(StoreError::Aborted);
        }
        let result = loadable(table).and_then(|t| self.check_finished(t));
        self.guard(result)
    }

    async fn commit(self: Box<Self>) -> Result<(), StoreError> {
        if self.poisoned {
            // Dropping the target rolls its transaction back.
            return Err(StoreError::Aborted);
        }
        for table in catalogue::TABLES {
            self.check_finished(table)?;
        }
        let this = *self;
        this.target.commit().await
    }
}

/// The catalogue's own entry for `table`, which must be exported.
fn loadable(table: &TableSpec) -> Result<&'static TableSpec, StoreError> {
    catalogue::check_exported(table)?;
    catalogue::table(table.name)
        .ok_or_else(|| StoreError::Invalid(format!("{}: not a catalogue table", table.name)))
}

fn position(table: &TableSpec) -> usize {
    catalogue::TABLES
        .iter()
        .position(|t| t.name == table.name)
        .unwrap_or(usize::MAX)
}

/// Checks and decodes one row: its columns in catalogue order.
fn prepare(table: &'static TableSpec, row: &Row) -> Result<Prepared, StoreError> {
    if let Some(unknown) = row.keys().find(|k| table.column(k).is_none()) {
        return Err(StoreError::Invalid(format!(
            "{}: unknown column {unknown}",
            table.name
        )));
    }
    if row.is_empty() {
        return Err(StoreError::Invalid(format!("{}: empty row", table.name)));
    }
    let mut columns = Vec::with_capacity(row.len());
    let mut cells = Vec::with_capacity(row.len());
    for (name, col) in table.columns {
        if let Some(value) = row.get(*name) {
            let cell = decode_column(table, name, *col, value)
                .map_err(|e| StoreError::Invalid(format!("{}.{name}: {e}", table.name)))?;
            columns.push((*name, *col));
            cells.push(cell);
        }
    }
    if let Export::OnlyKeys(allowed) = table.export {
        let key = table.key[0];
        let ok = matches!(
            row.get(key).and_then(|v| v.as_str()),
            Some(k) if allowed.contains(&k)
        );
        if !ok {
            return Err(StoreError::Invalid(format!(
                "{}: only the rows {allowed:?} are loaded",
                table.name
            )));
        }
    }
    Ok(Prepared { columns, cells })
}

/// The id cell of `column` in `row`, if set.
fn id_of(row: &Prepared, column: &str) -> Option<Uuid> {
    row.columns
        .iter()
        .position(|(c, _)| *c == column)
        .and_then(|i| match row.cells[i] {
            Cell::Id(id) => Some(id),
            _ => None,
        })
}

/// Orders the rows of a self-referencing table so each comes after its parent: rows
/// whose parent is not loaded yet wait in `tree` and are released after it.
fn order_tree(
    table: &TableSpec,
    parent_col: &str,
    tree: &mut Tree,
    rows: Vec<Prepared>,
) -> Result<Vec<Prepared>, StoreError> {
    // Self-referencing tables have one Id key, which the parent column references.
    let key = table.key[0];
    let mut ready = Vec::with_capacity(rows.len());
    for row in rows {
        let id = id_of(&row, key)
            .ok_or_else(|| StoreError::Invalid(format!("{}: row without {key}", table.name)))?;
        match id_of(&row, parent_col) {
            Some(parent) if !tree.loaded.contains(&parent) => {
                tree.waiting.entry(parent).or_default().push(row);
                continue;
            }
            _ => {}
        }
        // Insert it, then everything that waited for it (and for those, and so on).
        let mut released = vec![id];
        tree.loaded.insert(id);
        ready.push(row);
        while let Some(parent) = released.pop() {
            for child in tree.waiting.remove(&parent).unwrap_or_default() {
                if let Some(child_id) = id_of(&child, key) {
                    tree.loaded.insert(child_id);
                    released.push(child_id);
                }
                ready.push(child);
            }
        }
    }
    Ok(ready)
}

/// Cuts rows into batches of one column set, each at most [`MAX_LOAD_BATCH`] rows and
/// `max_params` bind parameters.
fn batches(rows: Vec<Prepared>, max_params: usize) -> Vec<InsertBatch> {
    let mut out: Vec<InsertBatch> = Vec::new();
    for row in rows {
        let limit = (max_params / row.columns.len().max(1)).clamp(1, MAX_LOAD_BATCH);
        match out.last_mut() {
            Some(batch) if batch.columns == row.columns && batch.rows.len() < limit => {
                batch.rows.push(row.cells);
            }
            _ => out.push(InsertBatch {
                columns: row.columns,
                rows: vec![row.cells],
            }),
        }
    }
    out
}

/// `INSERT INTO table (columns) VALUES (…), (…)` for `rows` rows; `mark(n)` gives the
/// placeholder of the n-th parameter (from 1). With `upsert`, `ON CONFLICT (key) DO
/// UPDATE` sets the non-key columns (both backends share the syntax).
pub fn insert_sql(
    table: &TableSpec,
    columns: &[(&'static str, ColType)],
    rows: usize,
    upsert: bool,
    mark: impl Fn(usize) -> String,
) -> String {
    let cols = columns
        .iter()
        .map(|(c, _)| quote(c))
        .collect::<Vec<_>>()
        .join(", ");
    let n = columns.len();
    let values = (0..rows)
        .map(|r| {
            let marks = (1..=n).map(|i| mark(r * n + i)).collect::<Vec<_>>();
            format!("({})", marks.join(", "))
        })
        .collect::<Vec<_>>()
        .join(", ");
    let mut sql = format!("INSERT INTO {} ({cols}) VALUES {values}", quote(table.name));
    if upsert {
        let key = table
            .key
            .iter()
            .map(|k| quote(k))
            .collect::<Vec<_>>()
            .join(", ");
        let set = columns
            .iter()
            .filter(|(c, _)| !table.key.contains(c))
            .map(|(c, _)| format!("{q} = excluded.{q}", q = quote(c)))
            .collect::<Vec<_>>();
        if set.is_empty() {
            sql.push_str(&format!(" ON CONFLICT ({key}) DO NOTHING"));
        } else {
            sql.push_str(&format!(
                " ON CONFLICT ({key}) DO UPDATE SET {}",
                set.join(", ")
            ));
        }
    }
    sql
}

/// `DELETE FROM table`, or with `keys` (a single-column key) only those rows; `mark(n)`
/// gives the n-th placeholder.
pub fn delete_sql(
    table: &TableSpec,
    keys: Option<&[&str]>,
    mark: impl Fn(usize) -> String,
) -> String {
    let mut sql = format!("DELETE FROM {}", quote(table.name));
    if let Some(keys) = keys {
        let marks = (1..=keys.len()).map(mark).collect::<Vec<_>>();
        sql.push_str(&format!(
            " WHERE {} IN ({})",
            quote(table.key[0]),
            marks.join(", ")
        ));
    }
    sql
}

/// Quotes a catalogue identifier. Not an escaper: callers pass only the static
/// catalogue's names, which never hold `"`.
fn quote(ident: &str) -> String {
    debug_assert!(!ident.contains('"'), "identifier with a quote: {ident}");
    format!("\"{ident}\"")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn row(v: serde_json::Value) -> Row {
        match v {
            serde_json::Value::Object(m) => m,
            _ => unreachable!(),
        }
    }

    fn spec(name: &str) -> &'static TableSpec {
        catalogue::table(name).unwrap()
    }

    #[test]
    fn self_referencing_tables_have_one_id_key() {
        for t in catalogue::TABLES.iter().filter(|t| t.self_ref.is_some()) {
            assert_eq!(t.key.len(), 1, "{}", t.name);
            assert_eq!(t.column(t.key[0]), Some(ColType::Id), "{}", t.name);
        }
    }

    #[test]
    fn prepare_refuses_unknown_columns_and_bad_values() {
        let t = spec("options");
        let ok = prepare(t, &row(json!({"value": 1, "key": "a"}))).unwrap();
        // Catalogue order, not the row's.
        assert_eq!(
            ok.columns,
            [("key", ColType::Text), ("value", ColType::Json)]
        );
        assert!(matches!(
            prepare(t, &row(json!({"key": "a", "nope": 1}))),
            Err(StoreError::Invalid(m)) if m.contains("unknown column nope")
        ));
        assert!(matches!(
            prepare(t, &row(json!({"key": "a", "autoload": 1}))),
            Err(StoreError::Invalid(_))
        ));
        assert!(matches!(
            prepare(t, &row(json!({}))),
            Err(StoreError::Invalid(_))
        ));
        let meta = spec("schema_meta");
        prepare(meta, &row(json!({"key": "site_id", "value": "x"}))).unwrap();
        assert!(matches!(
            prepare(
                meta,
                &row(json!({"key": "created_by_version", "value": "x"}))
            ),
            Err(StoreError::Invalid(_))
        ));
    }

    fn term(id: u128, parent: Option<u128>) -> Prepared {
        let t = spec("terms");
        let parent = parent.map(|p| Uuid::from_u128(p).hyphenated().to_string());
        prepare(
            t,
            &row(json!({"id": Uuid::from_u128(id).hyphenated().to_string(), "parent_id": parent})),
        )
        .unwrap()
    }

    #[test]
    fn order_tree_puts_parents_first_across_calls() {
        let t = spec("terms");
        let mut tree = Tree::default();
        // 3 -> 2 -> 1 (root), given leaf first; 4 waits for 9 (never given).
        let first = order_tree(
            t,
            "parent_id",
            &mut tree,
            vec![term(3, Some(2)), term(4, Some(9))],
        )
        .unwrap();
        assert!(first.is_empty());
        assert_eq!(tree.waiting_rows(), 2);
        let second = order_tree(
            t,
            "parent_id",
            &mut tree,
            vec![term(2, Some(1)), term(1, None), term(5, Some(1))],
        )
        .unwrap();
        let ids: Vec<Uuid> = second.iter().map(|r| id_of(r, "id").unwrap()).collect();
        assert_eq!(
            ids,
            [1, 2, 3, 5].map(Uuid::from_u128),
            "parent first, then the released subtree"
        );
        assert_eq!(tree.waiting_rows(), 1);
    }

    #[test]
    fn order_tree_holds_cycles_and_self_parents() {
        let t = spec("terms");
        let mut tree = Tree::default();
        // A -> B -> A, and C -> C: none can ever be inserted.
        let ready = order_tree(
            t,
            "parent_id",
            &mut tree,
            vec![term(10, Some(11)), term(11, Some(10)), term(12, Some(12))],
        )
        .unwrap();
        assert!(ready.is_empty());
        assert_eq!(tree.waiting_rows(), 3);
        assert!(tree.loaded.is_empty());
    }

    #[tokio::test]
    async fn waiting_rows_refuse_finish_and_commit() {
        struct NoTarget;
        #[async_trait]
        impl LoadTarget for NoTarget {
            fn max_params(&self) -> usize {
                100
            }
            async fn insert(
                &mut self,
                _: &'static TableSpec,
                _: InsertBatch,
                _: bool,
            ) -> Result<(), StoreError> {
                Ok(())
            }
            async fn delete(
                &mut self,
                _: &'static TableSpec,
                _: Option<&'static [&'static str]>,
            ) -> Result<(), StoreError> {
                Ok(())
            }
            async fn commit(self) -> Result<(), StoreError> {
                Ok(())
            }
        }
        let cycle = || {
            [(10, 11), (11, 10)]
                .map(|(id, parent)| {
                    row(json!({
                        "id": Uuid::from_u128(id).hyphenated().to_string(),
                        "parent_id": Uuid::from_u128(parent).hyphenated().to_string(),
                    }))
                })
                .to_vec()
        };
        let mut w: Box<dyn ImportWriter> = Box::new(Loader::new(NoTarget));
        w.rows(spec("terms"), cycle()).await.unwrap();
        assert!(matches!(
            w.finish_table(spec("terms")).await,
            Err(StoreError::Invalid(m)) if m.contains("2 rows wait")
        ));
        assert!(matches!(w.commit().await, Err(StoreError::Aborted)));
        let mut w: Box<dyn ImportWriter> = Box::new(Loader::new(NoTarget));
        w.rows(spec("terms"), cycle()).await.unwrap();
        assert!(matches!(w.commit().await, Err(StoreError::Invalid(_))));
    }

    #[test]
    fn batches_respect_rows_params_and_column_sets() {
        let t = spec("options");
        let r = |k: &str| prepare(t, &row(json!({"key": k, "value": 1}))).unwrap();
        let rows: Vec<Prepared> = (0..1001).map(|i| r(&i.to_string())).collect();
        let sizes: Vec<usize> = batches(rows, 65_535).iter().map(|b| b.rows.len()).collect();
        assert_eq!(sizes, [500, 500, 1]);
        let rows: Vec<Prepared> = (0..7).map(|i| r(&i.to_string())).collect();
        let sizes: Vec<usize> = batches(rows, 5).iter().map(|b| b.rows.len()).collect();
        assert_eq!(sizes, [2, 2, 2, 1], "two columns, five parameters");
        let mixed = vec![
            r("a"),
            prepare(t, &row(json!({"key": "b", "value": 1, "autoload": true}))).unwrap(),
            r("c"),
        ];
        assert_eq!(batches(mixed, 65_535).len(), 3);
    }

    #[test]
    fn sql_shapes() {
        let t = spec("schema_meta");
        let cols = [("key", ColType::Text), ("value", ColType::Text)];
        assert_eq!(
            insert_sql(t, &cols, 2, true, |n| format!("${n}")),
            "INSERT INTO \"schema_meta\" (\"key\", \"value\") VALUES ($1, $2), ($3, $4) \
             ON CONFLICT (\"key\") DO UPDATE SET \"value\" = excluded.\"value\""
        );
        assert_eq!(
            insert_sql(t, &cols[..1], 1, false, |_| "?".into()),
            "INSERT INTO \"schema_meta\" (\"key\") VALUES (?)"
        );
        assert_eq!(
            delete_sql(t, Some(&["site_id"]), |_| "?".into()),
            "DELETE FROM \"schema_meta\" WHERE \"key\" IN (?)"
        );
        assert_eq!(
            delete_sql(spec("users"), None, |_| "?".into()),
            "DELETE FROM \"users\""
        );
    }
}
