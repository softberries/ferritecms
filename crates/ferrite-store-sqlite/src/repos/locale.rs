//! Locale counts and relabels (design 13 §2.2).

use async_trait::async_trait;
use ferrite_store::{LocaleRepo, LocaleUsage, StoreError, validate_text};

use super::SqliteRepos;
use crate::exec::run;

#[async_trait]
impl LocaleRepo for SqliteRepos {
    async fn usage(&self, locale: &str) -> Result<LocaleUsage, StoreError> {
        validate_text("locale", locale)?;
        let (items, terms, menus): (i64, i64, i64) = run!(&self.reader, |e| sqlx::query_as(
            "SELECT \
               (SELECT COUNT(*) FROM content_items WHERE locale = ?1 AND type <> 'media'), \
               (SELECT COUNT(*) FROM terms WHERE locale = ?1), \
               (SELECT COUNT(*) FROM menus WHERE locale = ?1)"
        )
        .bind(locale)
        .fetch_one(e))?;
        Ok(LocaleUsage {
            items: u64::try_from(items).unwrap_or(0),
            terms: u64::try_from(terms).unwrap_or(0),
            menus: u64::try_from(menus).unwrap_or(0),
        })
    }

    async fn relabel_all(&self, to: &str) -> Result<LocaleUsage, StoreError> {
        validate_text("locale", to)?;
        relabel(self, to, "locale <> ?").await
    }

    async fn relabel_blank(&self, to: &str) -> Result<LocaleUsage, StoreError> {
        validate_text("locale", to)?;
        relabel(self, to, "locale = '' AND ? IS NOT NULL").await
    }

    async fn relabel_media(&self, to: &str) -> Result<u64, StoreError> {
        validate_text("locale", to)?;
        let done = run!(&self.writer, |e| sqlx::query(
            "UPDATE content_items SET locale = ?1 WHERE type = 'media' AND locale <> ?1"
        )
        .bind(to)
        .execute(e))?;
        Ok(done.rows_affected())
    }

    async fn fill_groups(&self) -> Result<u64, StoreError> {
        let items = run!(&self.writer, |e| sqlx::query(
            "UPDATE content_items SET translation_group = id WHERE translation_group IS NULL"
        )
        .execute(e))?;
        let terms = run!(&self.writer, |e| sqlx::query(
            "UPDATE terms SET translation_group = id WHERE translation_group IS NULL"
        )
        .execute(e))?;
        Ok(items.rows_affected() + terms.rows_affected())
    }
}

/// `UPDATE <table> SET locale = ? WHERE <condition>` on the three tables; `condition`
/// takes `to` as its one parameter.
async fn relabel(
    repos: &SqliteRepos,
    to: &str,
    condition: &'static str,
) -> Result<LocaleUsage, StoreError> {
    let mut changed = [0u64; 3];
    for (n, table) in ["content_items", "terms", "menus"].into_iter().enumerate() {
        let sql = format!("UPDATE {table} SET locale = ? WHERE {condition}");
        let done = run!(&repos.writer, |e| sqlx::query(sqlx::AssertSqlSafe(
            sql.clone()
        ))
        .bind(to)
        .bind(to)
        .execute(e))?;
        changed[n] = done.rows_affected();
    }
    Ok(LocaleUsage {
        items: changed[0],
        terms: changed[1],
        menus: changed[2],
    })
}
